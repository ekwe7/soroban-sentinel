use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::domain::RiskLevel;
use crate::error::{SentinelError, SentinelResult};
use crate::ports::{AlertKind, AlertMinLevel, AlertNotifier};
use crate::usecases::TTLReport;

const MAX_ALERT_ENTRIES: usize = 100;

pub struct WebhookAlertNotifier {
    url: String,
    client: reqwest::Client,
}

impl WebhookAlertNotifier {
    pub fn new(url: impl Into<String>) -> SentinelResult<Self> {
        let url = url.into();
        let parsed = reqwest::Url::parse(&url).map_err(|_| invalid_url())?;
        let is_loopback = parsed.host_str().is_some_and(|host| {
            host.eq_ignore_ascii_case("localhost") || host == "127.0.0.1" || host == "::1"
        });
        if parsed.scheme() != "https" && !is_loopback {
            return Err(invalid_url());
        }
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| {
                SentinelError::SerializationError("could not configure alert client".into())
            })?;
        Ok(Self { url, client })
    }

    fn payload(report: &TTLReport, kind: AlertKind, min_level: AlertMinLevel) -> Option<Value> {
        if !min_level.includes(report.overall_status) {
            return None;
        }
        let entries = report
            .entries
            .iter()
            .filter(|entry| min_level.includes(entry.status))
            .take(MAX_ALERT_ENTRIES)
            .map(|entry| {
                json!({
                    "key_identifier": entry.key_identifier,
                    "status": entry.status.to_string(),
                    "remaining_ledgers": entry.remaining_ledgers,
                    "remaining_days": entry.remaining_days,
                })
            })
            .collect::<Vec<_>>();
        let total = report
            .entries
            .iter()
            .filter(|entry| min_level.includes(entry.status))
            .count();
        let summary = json!({
            "contract_id": report.contract_id,
            "network": report.network,
            "overall_status": report.overall_status.to_string(),
            "entries": entries,
            "omitted_entries": total.saturating_sub(MAX_ALERT_ENTRIES),
        });
        Some(match kind {
            AlertKind::Slack => json!({
                "text": format!("Soroban Sentinel alert: {} ({})", report.contract_id, report.network),
                "blocks": [{
                    "type": "section",
                    "text": {"type": "mrkdwn", "text": format!("Soroban Sentinel alert\nContract: {}\nNetwork: {}\nStatus: {}", report.contract_id, report.network, report.overall_status)}
                }, {
                    "type": "section",
                    "text": {"type": "mrkdwn", "text": serde_json::to_string(&summary).unwrap_or_default()}
                }]
            }),
            AlertKind::Discord => json!({
                "embeds": [{
                    "title": "Soroban Sentinel alert",
                    "description": serde_json::to_string(&summary).unwrap_or_default(),
                    "color": if report.overall_status == RiskLevel::Archived || report.overall_status == RiskLevel::Critical { 15158332 } else { 16776960 }
                }]
            }),
        })
    }
}

#[async_trait]
impl AlertNotifier for WebhookAlertNotifier {
    async fn notify(
        &self,
        report: &TTLReport,
        kind: AlertKind,
        min_level: AlertMinLevel,
    ) -> SentinelResult<bool> {
        let Some(payload) = Self::payload(report, kind, min_level) else {
            return Ok(false);
        };
        let response = self
            .client
            .post(&self.url)
            .json(&payload)
            .send()
            .await
            .map_err(|_| SentinelError::RpcConnectionTimeout("alert delivery failed".into()))?;
        if !response.status().is_success() {
            return Err(SentinelError::RpcResponseError {
                code: i64::from(response.status().as_u16()),
                message: "alert delivery failed".into(),
            });
        }
        Ok(true)
    }
}

fn invalid_url() -> SentinelError {
    SentinelError::InvalidRpcUrl(
        "alert webhook must use HTTPS, except loopback test endpoints".into(),
    )
}
