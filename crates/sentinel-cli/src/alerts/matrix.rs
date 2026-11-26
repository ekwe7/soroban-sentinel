use std::{
    collections::HashMap,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use reqwest::{Client, Url};
use sentinel_core::{RiskLevel, TTLReport};
use serde_json::json;

static NEXT_TRANSACTION_ID: AtomicU64 = AtomicU64::new(0);

/// Sends formatted notices to a Matrix room using the Client-Server API.
pub struct MatrixAlertDispatcher {
    homeserver: Url,
    room_id: String,
    access_token: String,
    client: Client,
    reported: HashMap<String, RiskLevel>,
    pending: HashMap<String, (RiskLevel, String)>,
}

impl MatrixAlertDispatcher {
    /// Creates a dispatcher for a Matrix homeserver, room, and access token.
    pub fn new(
        homeserver: &str,
        room_id: impl Into<String>,
        access_token: impl Into<String>,
    ) -> Result<Self, String> {
        let homeserver =
            Url::parse(homeserver).map_err(|_| "Invalid Matrix homeserver URL".to_string())?;
        let host = homeserver.host_str();
        let local_http = homeserver.scheme() == "http"
            && matches!(host, Some("localhost" | "127.0.0.1" | "::1"));
        if (homeserver.scheme() != "https" && !local_http)
            || !homeserver.username().is_empty()
            || homeserver.password().is_some()
            || homeserver.query().is_some()
            || homeserver.fragment().is_some()
        {
            return Err("Invalid Matrix homeserver URL".into());
        }
        let room_id = room_id.into();
        let access_token = access_token.into();
        if !room_id.starts_with('!') || !room_id.contains(':') || access_token.trim().is_empty() {
            return Err("Matrix room ID and access token must be valid and nonempty".into());
        }

        let client = Client::builder()
            .timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| "Could not initialize Matrix HTTP client".to_string())?;

        Ok(Self {
            homeserver,
            room_id,
            access_token,
            client,
            reported: HashMap::new(),
            pending: HashMap::new(),
        })
    }

    /// Sends notices for entries whose risk status has changed.
    pub async fn observe(&mut self, report: &TTLReport) -> Result<(), String> {
        let mut errors = Vec::new();

        for entry in &report.entries {
            let key = format!(
                "{}:{}:{}",
                report.network, report.contract_id, entry.key_identifier
            );
            if entry.status == RiskLevel::Ok {
                self.reported.remove(&key);
                self.pending.remove(&key);
                continue;
            }
            if self.reported.get(&key) == Some(&entry.status) {
                continue;
            }

            let transaction_id = match self.pending.get(&key) {
                Some((status, transaction_id)) if *status == entry.status => transaction_id.clone(),
                _ => {
                    let transaction_id = transaction_id();
                    self.pending
                        .insert(key.clone(), (entry.status, transaction_id.clone()));
                    transaction_id
                }
            };
            let mut url = self.homeserver.clone();
            let Ok(mut path) = url.path_segments_mut() else {
                return Err("Invalid Matrix homeserver URL path".into());
            };
            path.pop_if_empty().extend([
                "_matrix",
                "client",
                "v3",
                "rooms",
                self.room_id.as_str(),
                "send",
                "m.room.message",
                transaction_id.as_str(),
            ]);
            drop(path);

            let (body, formatted_body) = message_bodies(report, entry);
            let response = self
                .client
                .put(url)
                .bearer_auth(&self.access_token)
                .json(&json!({
                    "msgtype": "m.notice",
                    "body": body,
                    "format": "org.matrix.custom.html",
                    "formatted_body": formatted_body
                }))
                .send()
                .await;

            match response {
                Ok(response) if response.status().is_success() => {
                    self.reported.insert(key.clone(), entry.status);
                    self.pending.remove(&key);
                }
                Ok(response) => errors.push(format!(
                    "Matrix homeserver returned HTTP {}",
                    response.status()
                )),
                Err(_) => errors.push("Matrix notification request failed".into()),
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join(", "))
        }
    }
}

fn transaction_id() -> String {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let sequence = NEXT_TRANSACTION_ID.fetch_add(1, Ordering::Relaxed);
    format!("sentinel-{timestamp}-{sequence}")
}

fn message_bodies(
    report: &TTLReport,
    entry: &sentinel_core::StorageEntryReport,
) -> (String, String) {
    let badge = match entry.status {
        RiskLevel::Ok => "🟢 OK",
        RiskLevel::Warn => "🟠 WARN",
        RiskLevel::Critical => "🔴 CRITICAL",
        RiskLevel::Archived => "⚫ ARCHIVED",
    };
    let body = format!(
        "Soroban Sentinel {badge}: contract {} has {} days remaining (live until ledger {}).",
        report.contract_id, entry.remaining_days, entry.live_until_ledger
    );
    let explorer_url = format!(
        "https://stellar.expert/explorer/{}/contract/{}",
        report.network, report.contract_id
    );
    let formatted_body = format!(
        "<strong>Soroban Sentinel {}</strong><br>\
         Contract: <a href=\"{}\">{}</a><br>\
         Status: {}<br>\
         Estimated days remaining: {:.1}<br>\
         Live until ledger: {}",
        html_escape(&entry.status.to_string()),
        html_escape(&explorer_url),
        html_escape(&report.contract_id),
        html_escape(badge),
        entry.remaining_days,
        entry.live_until_ledger,
    );
    (body, formatted_body)
}

fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
