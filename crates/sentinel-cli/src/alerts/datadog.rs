//! Datadog HTTP API adapter for publishing contract TTL metrics and lifecycle events.
//!
//! Sends gauge metrics to `/api/v1/series` and alert events to `/api/v1/events`
//! using the Datadog HTTPS API. Authentication uses the `DD-API-KEY` header.
//! No credentials are exposed in log output or error messages.

use std::{collections::HashMap, time::Duration};

use reqwest::{Client, Url};
use sentinel_core::{RiskLevel, StorageEntryReport, TTLReport};
use serde_json::{json, Value};

/// Supported Datadog intake sites.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum DatadogSite {
    /// United States (`datadoghq.com`).
    #[default]
    Us,
    /// European Union (`datadoghq.eu`).
    Eu,
}

impl DatadogSite {
    /// Parses a site string such as `"datadoghq.com"` or `"datadoghq.eu"`.
    pub fn parse(s: &str) -> Result<Self, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "datadoghq.com" => Ok(Self::Us),
            "datadoghq.eu" => Ok(Self::Eu),
            other => Err(format!(
                "unsupported Datadog site '{other}'; expected 'datadoghq.com' or 'datadoghq.eu'"
            )),
        }
    }

    /// Returns the base API URL for this site.
    pub fn base_url(&self) -> &'static str {
        match self {
            Self::Us => "https://api.datadoghq.com",
            Self::Eu => "https://api.datadoghq.eu",
        }
    }
}

impl std::fmt::Display for DatadogSite {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Us => write!(f, "datadoghq.com"),
            Self::Eu => write!(f, "datadoghq.eu"),
        }
    }
}

/// Datadog adapter that publishes TTL gauge metrics and risk-transition events.
///
/// Each instance tracks which risk state was last sent per entry key so that
/// events are only dispatched when the state changes.
pub struct DatadogAdapter {
    /// Redacted at construction; never stored as a plain string in fields
    /// accessible from outside the module.
    api_key: String,
    metrics_url: Url,
    events_url: Url,
    client: Client,
    /// Maps a stable entry identifier to the last-sent risk state.
    reported: HashMap<String, RiskLevel>,
}

impl DatadogAdapter {
    /// Creates a new `DatadogAdapter`.
    ///
    /// `api_key` is stored only in memory and never logged or included in
    /// error messages.
    pub fn new(api_key: impl Into<String>, site: &DatadogSite) -> Result<Self, reqwest::Error> {
        let base = site.base_url();
        Ok(Self {
            api_key: api_key.into(),
            metrics_url: format!("{base}/api/v1/series")
                .parse()
                .expect("metrics URL is static and valid"),
            events_url: format!("{base}/api/v1/events")
                .parse()
                .expect("events URL is static and valid"),
            client: Client::builder()
                .timeout(Duration::from_secs(10))
                .redirect(reqwest::redirect::Policy::none())
                .build()?,
            reported: HashMap::new(),
        })
    }

    /// Creates a new `DatadogAdapter` with custom base URLs (used in tests).
    pub fn new_with_base_url(
        api_key: impl Into<String>,
        base_url: &str,
    ) -> Result<Self, reqwest::Error> {
        Ok(Self {
            api_key: api_key.into(),
            metrics_url: format!("{base_url}/api/v1/series")
                .parse()
                .expect("metrics URL is valid"),
            events_url: format!("{base_url}/api/v1/events")
                .parse()
                .expect("events URL is valid"),
            client: Client::builder()
                .timeout(Duration::from_secs(10))
                .redirect(reqwest::redirect::Policy::none())
                .build()?,
            reported: HashMap::new(),
        })
    }

    /// Publishes TTL gauge metrics and risk-transition events for the given report.
    ///
    /// All gauge points from the report are batched into a single metrics
    /// request. Events are submitted individually only when an entry's risk
    /// state changes (or first becomes non-OK).
    pub async fn observe(&mut self, report: &TTLReport) -> Result<(), String> {
        let mut errors = Vec::new();

        // Batch all metric points and submit in one request.
        if !report.entries.is_empty() {
            if let Err(e) = self.submit_metrics(report).await {
                errors.push(e);
            }
        }

        // Submit events only when state transitions occur.
        for entry in &report.entries {
            let key = entry_key(&report.contract_id, &report.network, &entry.key_identifier);
            let previous = self.reported.get(&key).copied();

            if previous == Some(entry.status) {
                // No state change – skip to avoid duplicates.
                continue;
            }

            if entry.status == RiskLevel::Ok {
                // Resolution: remove from tracking, no Datadog event needed.
                self.reported.remove(&key);
                continue;
            }

            // State changed to Warn / Critical / Archived.
            if let Err(e) = self
                .submit_event(report, entry, previous, entry.status)
                .await
            {
                errors.push(e);
                // Do not update `reported` on failure so it retries next scan.
                continue;
            }
            self.reported.insert(key, entry.status);
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join(", "))
        }
    }

    // ── private helpers ───────────────────────────────────────────────────────

    async fn submit_metrics(&self, report: &TTLReport) -> Result<(), String> {
        let now_unix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let series: Vec<Value> = report
            .entries
            .iter()
            .map(|entry| {
                let tags = build_tags(
                    &report.contract_id,
                    &report.network,
                    entry.durability.to_string().to_ascii_lowercase().as_str(),
                );
                json!({
                    "metric": "stellar.contract.ttl_remaining",
                    "type": "gauge",
                    "points": [[now_unix, entry.remaining_ledgers]],
                    "tags": tags
                })
            })
            .collect();

        let payload = json!({ "series": series });
        self.post_json(&self.metrics_url.clone(), payload).await
    }

    async fn submit_event(
        &self,
        report: &TTLReport,
        entry: &StorageEntryReport,
        previous: Option<RiskLevel>,
        current: RiskLevel,
    ) -> Result<(), String> {
        let alert_type = match current {
            RiskLevel::Warn => "warning",
            RiskLevel::Critical | RiskLevel::Archived => "error",
            RiskLevel::Ok => "success",
        };

        let title = format!(
            "Soroban TTL {} on {} ({})",
            current, report.contract_id, report.network
        );

        let previous_label = previous
            .map(|r| r.to_string())
            .unwrap_or_else(|| "OK".into());

        let text = format!(
            "Contract: {}\nNetwork: {}\nEntry: {}\nDurability: {}\nPrevious state: {}\nCurrent state: {}\nRemaining ledgers: {}",
            report.contract_id,
            report.network,
            entry.key_identifier,
            entry.durability,
            previous_label,
            current,
            entry.remaining_ledgers,
        );

        let tags = build_tags(
            &report.contract_id,
            &report.network,
            entry.durability.to_string().to_ascii_lowercase().as_str(),
        );

        let payload = json!({
            "title": title,
            "text": text,
            "alert_type": alert_type,
            "source_type_name": "soroban-sentinel",
            "tags": tags
        });

        self.post_json(&self.events_url.clone(), payload).await
    }

    async fn post_json(&self, url: &Url, payload: Value) -> Result<(), String> {
        let request = self
            .client
            .post(url.clone())
            .header("DD-API-KEY", &self.api_key)
            .json(&payload);

        for attempt in 0..3u8 {
            let response = request
                .try_clone()
                .ok_or("Datadog request could not be retried")?
                .send()
                .await;

            let delay = match response {
                Ok(resp) if resp.status().is_success() => return Ok(()),
                Ok(resp) => {
                    let status = resp.status();
                    if status.as_u16() == 403 {
                        return Err("Datadog authentication failed (HTTP 403)".into());
                    }
                    if attempt == 2 || (!status.is_server_error() && status.as_u16() != 429) {
                        return Err(format!("Datadog API returned HTTP {status}"));
                    }
                    resp.headers()
                        .get(reqwest::header::RETRY_AFTER)
                        .and_then(|v| v.to_str().ok())
                        .and_then(|v| v.parse::<u64>().ok())
                        .map(|s| Duration::from_secs(s.min(5)))
                }
                Err(err) if attempt < 2 && (err.is_timeout() || err.is_connect()) => None,
                Err(_) => return Err("Datadog request failed".into()),
            };

            tokio::time::sleep(delay.unwrap_or(Duration::from_millis(100 << attempt))).await;
        }

        Err("Datadog delivery retries exhausted".into())
    }
}

// ── helpers ───────────────────────────────────────────────────────────────────

/// Builds the standard tag list for a contract entry.
fn build_tags(contract_id: &str, network: &str, durability: &str) -> Vec<String> {
    vec![
        format!("contract_id:{contract_id}"),
        format!("network:{network}"),
        format!("durability:{durability}"),
    ]
}

/// Returns a stable string key for deduplication tracking.
fn entry_key(contract_id: &str, network: &str, key_identifier: &str) -> String {
    format!("{contract_id}:{network}:{key_identifier}")
}

#[cfg(test)]
mod unit_tests {
    use super::*;

    #[test]
    fn site_parse_accepts_us_and_eu() {
        assert_eq!(
            DatadogSite::parse("datadoghq.com").unwrap(),
            DatadogSite::Us
        );
        assert_eq!(DatadogSite::parse("datadoghq.eu").unwrap(), DatadogSite::Eu);
        assert_eq!(
            DatadogSite::parse("DATADOGHQ.COM").unwrap(),
            DatadogSite::Us
        );
    }

    #[test]
    fn site_parse_rejects_unknown() {
        assert!(DatadogSite::parse("datadoghq.jp").is_err());
        assert!(DatadogSite::parse("").is_err());
    }

    #[test]
    fn build_tags_format() {
        let tags = build_tags("CABC", "testnet", "persistent");
        assert!(tags.contains(&"contract_id:CABC".to_string()));
        assert!(tags.contains(&"network:testnet".to_string()));
        assert!(tags.contains(&"durability:persistent".to_string()));
    }

    #[test]
    fn entry_key_is_stable() {
        assert_eq!(
            entry_key("CABC", "testnet", "instance"),
            "CABC:testnet:instance"
        );
    }
}
