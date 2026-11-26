use std::{collections::HashMap, time::Duration};

use reqwest::{Client, Url};
use sentinel_core::{RiskLevel, TTLReport};
use serde_json::json;
use sha2::{Digest, Sha256};

pub enum Provider {
    PagerDuty { endpoint: Url, routing_key: String },
    Opsgenie { endpoint: Url, api_key: String },
}

/// Creates a stable identifier within both providers' length limits.
pub fn dedup_key(network: &str, contract_id: &str, key_xdr: &str) -> String {
    let identity = format!("{}:{contract_id}:{key_xdr}", network.to_ascii_lowercase());
    format!("{:x}", Sha256::digest(identity.as_bytes()))
}

pub struct IncidentDispatcher {
    provider: Provider,
    client: Client,
    reported: HashMap<String, &'static str>,
}

impl IncidentDispatcher {
    pub fn new(provider: Provider) -> Result<Self, reqwest::Error> {
        Ok(Self {
            provider,
            client: Client::builder()
                .timeout(Duration::from_secs(5))
                .redirect(reqwest::redirect::Policy::none())
                .build()?,
            reported: HashMap::new(),
        })
    }

    /// Updates each observed key independently. An unobserved key is not a
    /// renewal. Failed deliveries remain eligible on the next scan.
    pub async fn observe(&mut self, report: &TTLReport, keys: &[String]) -> Result<(), String> {
        if report.entries.len() != keys.len() {
            return Err("Report entries and ledger keys differ in count".into());
        }
        let mut errors = Vec::new();
        for (entry, key) in report.entries.iter().zip(keys) {
            let key = dedup_key(&report.network, &report.contract_id, key);
            let severity = match entry.status {
                RiskLevel::Ok => None,
                RiskLevel::Warn => Some("warning"),
                RiskLevel::Critical | RiskLevel::Archived => Some("critical"),
            };
            if self.reported.get(&key).copied() == severity {
                continue;
            }
            let summary = format!(
                "Soroban TTL risk on {} ({})",
                report.contract_id, report.network
            );
            if let Err(error) = self.dispatch(&key, severity, &summary).await {
                errors.push(error);
                continue;
            }
            if let Some(severity) = severity {
                self.reported.insert(key, severity);
            } else {
                self.reported.remove(&key);
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join(", "))
        }
    }

    async fn dispatch(
        &self,
        key: &str,
        severity: Option<&str>,
        summary: &str,
    ) -> Result<(), String> {
        let request = match &self.provider {
            Provider::PagerDuty {
                endpoint,
                routing_key,
            } => {
                let payload = if let Some(severity) = severity {
                    json!({
                        "routing_key": routing_key, "event_action": "trigger", "dedup_key": key,
                        "payload": {"summary": summary, "source": "soroban-sentinel", "severity": severity},
                    })
                } else {
                    json!({"routing_key": routing_key, "event_action": "resolve", "dedup_key": key})
                };
                self.client.post(endpoint.clone()).json(&payload)
            }
            Provider::Opsgenie { endpoint, api_key } => {
                let request = if let Some(severity) = severity {
                    let priority = if severity == "critical" { "P2" } else { "P3" };
                    self.client.post(endpoint.clone()).json(&json!({
                        "message": summary.chars().take(130).collect::<String>(), "alias": key, "source": "soroban-sentinel", "priority": priority,
                    }))
                } else {
                    let mut url = endpoint.clone();
                    url.path_segments_mut()
                        .map_err(|_| "Invalid Opsgenie alert endpoint")?
                        .pop_if_empty()
                        .extend([key, "close"]);
                    url.query_pairs_mut().append_pair("identifierType", "alias");
                    self.client
                        .post(url)
                        .json(&json!({"source": "soroban-sentinel"}))
                };
                request.header(
                    reqwest::header::AUTHORIZATION,
                    format!("GenieKey {api_key}"),
                )
            }
        };
        for attempt in 0..3 {
            let response = request
                .try_clone()
                .ok_or("Incident request could not be retried")?
                .send()
                .await;
            let delay = match response {
                Ok(response) if response.status().is_success() => return Ok(()),
                Ok(response) => {
                    let status = response.status();
                    if attempt == 2 || (!status.is_server_error() && status.as_u16() != 429) {
                        return Err(format!("Incident provider returned HTTP {status}"));
                    }
                    response
                        .headers()
                        .get(reqwest::header::RETRY_AFTER)
                        .and_then(|value| value.to_str().ok())
                        .and_then(|value| value.parse::<u64>().ok())
                        .map(|seconds| Duration::from_secs(seconds.min(5)))
                }
                Err(error) if attempt < 2 && (error.is_timeout() || error.is_connect()) => None,
                Err(_) => return Err("Incident provider request failed".into()),
            };
            tokio::time::sleep(delay.unwrap_or(Duration::from_millis(100 << attempt))).await;
        }
        Err("Incident delivery retries exhausted".into())
    }
}
