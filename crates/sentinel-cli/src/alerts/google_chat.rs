use std::{collections::HashMap, time::Duration};

use reqwest::{Client, Url};
use sentinel_core::{RiskLevel, TTLReport};
use serde_json::{json, Value};

/// Sends risk-transition alerts to a Google Chat incoming webhook.
pub struct GoogleChatAlertDispatcher {
    webhook_url: Url,
    client: Client,
    reported: HashMap<String, RiskLevel>,
}

impl GoogleChatAlertDispatcher {
    /// Creates a dispatcher for a Google Chat incoming webhook URL.
    pub fn new(webhook_url: &str) -> Result<Self, String> {
        let webhook_url =
            Url::parse(webhook_url).map_err(|_| "Invalid Google Chat webhook URL".to_string())?;
        let host = webhook_url.host_str();
        let local_http = webhook_url.scheme() == "http"
            && matches!(host, Some("localhost" | "127.0.0.1" | "::1"));
        if webhook_url.scheme() != "https" && !local_http {
            return Err("Invalid Google Chat webhook URL".into());
        }

        let client = Client::builder()
            .timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| "Could not initialize Google Chat HTTP client".to_string())?;

        Ok(Self {
            webhook_url,
            client,
            reported: HashMap::new(),
        })
    }

    /// Sends alerts for entries whose risk status has changed.
    pub async fn observe(&mut self, report: &TTLReport) -> Result<(), String> {
        let mut errors = Vec::new();

        for entry in &report.entries {
            let key = format!(
                "{}:{}:{}",
                report.network, report.contract_id, entry.key_identifier
            );

            if entry.status == RiskLevel::Ok {
                self.reported.remove(&key);
                continue;
            }
            if self.reported.get(&key) == Some(&entry.status) {
                continue;
            }

            let payload = card_payload(report, entry);
            match self
                .client
                .post(self.webhook_url.clone())
                .json(&payload)
                .send()
                .await
            {
                Ok(response) if response.status().is_success() => {
                    self.reported.insert(key, entry.status);
                }
                Ok(response) => errors.push(format!(
                    "Google Chat webhook returned HTTP {}",
                    response.status()
                )),
                Err(_) => errors.push("Google Chat webhook request failed".into()),
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join(", "))
        }
    }
}

fn card_payload(report: &TTLReport, entry: &sentinel_core::StorageEntryReport) -> Value {
    let badge = match entry.status {
        RiskLevel::Ok => "🟢 OK",
        RiskLevel::Warn => "🟠 WARN",
        RiskLevel::Critical => "🔴 CRITICAL",
        RiskLevel::Archived => "⚫ ARCHIVED",
    };
    let explorer_url = format!(
        "https://stellar.expert/explorer/{}/contract/{}",
        report.network, report.contract_id
    );

    json!({
        "cardsV2": [{
            "cardId": "soroban-sentinel-alert",
            "card": {
                "header": {
                    "title": format!("Soroban Sentinel: {}", entry.status),
                    "subtitle": report.contract_id
                },
                "sections": [{
                    "widgets": [
                        {
                            "decoratedText": {
                                "topLabel": "Contract",
                                "text": report.contract_id
                            }
                        },
                        {
                            "decoratedText": {
                                "topLabel": "Status",
                                "text": badge
                            }
                        },
                        {
                            "decoratedText": {
                                "topLabel": "Estimated days remaining",
                                "text": format!("{:.1}", entry.remaining_days)
                            }
                        },
                        {
                            "decoratedText": {
                                "topLabel": "Live until ledger",
                                "text": entry.live_until_ledger.to_string()
                            }
                        },
                        {
                            "buttonList": {
                                "buttons": [{
                                    "text": "View on Stellar Expert",
                                    "onClick": {
                                        "openLink": { "url": explorer_url }
                                    }
                                }]
                            }
                        }
                    ]
                }]
            }
        }]
    })
}
