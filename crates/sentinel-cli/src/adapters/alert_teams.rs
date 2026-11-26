//! Microsoft Teams incoming webhook dispatcher using Adaptive Cards 1.5.

use std::{collections::HashSet, time::Duration};

use reqwest::{Client, Url};
use sentinel_core::{RiskLevel, StorageEntryReport, TTLReport};
use serde_json::{json, Value};

/// Posts newly observed non-OK storage risks to a Teams incoming webhook.
pub struct TeamsAlertDispatcher {
    webhook_url: Url,
    client: Client,
    reported: HashSet<String>,
}

impl TeamsAlertDispatcher {
    pub fn new(webhook_url: &str) -> Result<Self, String> {
        let webhook_url = Url::parse(webhook_url).map_err(|_| "invalid Teams webhook URL")?;
        if !matches!(webhook_url.scheme(), "http" | "https") || webhook_url.host_str().is_none() {
            return Err("invalid Teams webhook URL".into());
        }
        Self::with_url(webhook_url)
    }

    /// Custom endpoint constructor for local HTTP tests.
    pub fn new_with_webhook_url(webhook_url: &str) -> Result<Self, String> {
        Self::new(webhook_url)
    }

    fn with_url(webhook_url: Url) -> Result<Self, String> {
        let client = Client::builder()
            .timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| "could not initialize Teams HTTP client")?;
        Ok(Self {
            webhook_url,
            client,
            reported: HashSet::new(),
        })
    }

    /// Dispatches one Adaptive Card for each newly breached entry.
    /// Successfully delivered risks are deduplicated for this process lifetime.
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
            if self.reported.contains(&key) {
                continue;
            }
            match self.send_card(report, entry).await {
                Ok(()) => {
                    self.reported.insert(key);
                }
                Err(error) => errors.push(error),
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join(", "))
        }
    }

    async fn send_card(
        &self,
        report: &TTLReport,
        entry: &StorageEntryReport,
    ) -> Result<(), String> {
        let payload = teams_payload(report, entry);
        let response = self
            .client
            .post(self.webhook_url.clone())
            .json(&payload)
            .send()
            .await
            .map_err(|_| "Teams webhook request failed")?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(format!("Teams webhook returned HTTP {}", response.status()))
        }
    }
}

fn teams_payload(report: &TTLReport, entry: &StorageEntryReport) -> Value {
    let (color, severity) = match entry.status {
        RiskLevel::Warn => ("Warning", "Warning"),
        RiskLevel::Critical => ("Attention", "Critical"),
        RiskLevel::Archived => ("Dark", "Archived"),
        RiskLevel::Ok => ("Good", "OK"),
    };
    let explorer_network = match report.network.to_ascii_lowercase().as_str() {
        "mainnet" | "pubnet" => "public",
        "testnet" => "testnet",
        "futurenet" => "futurenet",
        _ => "testnet",
    };
    let url = format!(
        "https://stellar.expert/explorer/{explorer_network}/contract/{}",
        report.contract_id
    );
    json!({
        "type": "message",
        "attachments": [{
            "contentType": "application/vnd.microsoft.card.adaptive",
            "contentUrl": null,
            "content": {
                "$schema": "https://adaptivecards.io/schemas/adaptive-card.json",
                "type": "AdaptiveCard",
                "version": "1.5",
                "body": [
                    { "type": "Container", "style": color, "bleed": true, "items": [
                        { "type": "TextBlock", "text": format!("Soroban TTL {severity}"), "weight": "Bolder", "size": "Large", "color": "Light", "wrap": true }
                    ]},
                    { "type": "FactSet", "facts": [
                        { "title": "Contract ID", "value": report.contract_id },
                        { "title": "Network", "value": report.network },
                        { "title": "Entry", "value": entry.key_identifier },
                        { "title": "Live until ledger", "value": entry.live_until_ledger.to_string() },
                        { "title": "Remaining ledgers", "value": entry.remaining_ledgers.to_string() }
                    ]}
                ],
                "actions": [{ "type": "Action.OpenUrl", "title": "View on Stellar Expert", "url": url }]
            }
        }]
    })
}
