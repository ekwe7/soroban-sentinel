//! Telegram Bot API outbound alert dispatcher.
//!
//! Consumes [`SentinelEvent::RiskThresholdBreached`] events and dispatches
//! HTML-formatted alerts to a configured Telegram chat or channel via the
//! Telegram Bot API `sendMessage` endpoint.

use std::env;
use std::fmt;
use std::time::Duration;

use reqwest::{Client, Url};
use sentinel_core::{RiskLevel, StorageEntry, TTLReport};
use serde::Serialize;
use tokio::sync::mpsc;
use zeroize::Zeroizing;

/// Default Telegram Bot API base URL.
pub const TELEGRAM_API_BASE_URL: &str = "https://api.telegram.org";

/// Environment variable supplying the Telegram Bot API token.
pub const ENV_TELEGRAM_BOT_TOKEN: &str = "SENTINEL_TELEGRAM_BOT_TOKEN";

/// Environment variable supplying the target Telegram chat or channel ID.
pub const ENV_TELEGRAM_CHAT_ID: &str = "SENTINEL_TELEGRAM_CHAT_ID";

const DEFAULT_TIMEOUT_SECS: u64 = 10;
const MAX_DISPATCH_ATTEMPTS: usize = 3;
const MAX_KEY_DISPLAY_CHARS: usize = 256;

/// Domain events emitted during continuous Soroban contract monitoring.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SentinelEvent {
    /// A new ledger sequence closed on the monitored network.
    LedgerTicked { sequence: u32, close_time: u64 },
    /// A storage entry was evaluated during a scan cycle.
    EntryEvaluated {
        contract_id: String,
        network: String,
        entry: StorageEntry,
        remaining_ledgers: i64,
        risk: RiskLevel,
    },
    /// A storage entry breached a warning, critical, or archival risk threshold.
    RiskThresholdBreached {
        contract_id: String,
        network: String,
        entry: StorageEntry,
        remaining_ledgers: i64,
        risk: RiskLevel,
    },
    /// An RPC polling attempt failed.
    RpcPollFailed {
        error: String,
        consecutive_failures: u32,
    },
}

impl SentinelEvent {
    /// Constructs a [`SentinelEvent::RiskThresholdBreached`] event.
    pub fn risk_threshold_breached(
        contract_id: impl Into<String>,
        network: impl Into<String>,
        entry: StorageEntry,
        remaining_ledgers: i64,
        risk: RiskLevel,
    ) -> Self {
        Self::RiskThresholdBreached {
            contract_id: contract_id.into(),
            network: network.into(),
            entry,
            remaining_ledgers,
            risk,
        }
    }

    /// Converts all non-`Ok` entries in a [`TTLReport`] into
    /// [`SentinelEvent::RiskThresholdBreached`] events.
    pub fn breaches_from_report(report: &TTLReport) -> Vec<Self> {
        report
            .entries
            .iter()
            .filter(|entry| entry.status != RiskLevel::Ok)
            .map(|entry| {
                let storage_entry = StorageEntry::new(
                    &entry.key_identifier,
                    entry.entry_type,
                    entry.durability,
                    entry.byte_size,
                    entry.live_until_ledger,
                    report.latest_ledger,
                );
                Self::RiskThresholdBreached {
                    contract_id: report.contract_id.clone(),
                    network: report.network.clone(),
                    entry: storage_entry,
                    remaining_ledgers: entry.remaining_ledgers,
                    risk: entry.status,
                }
            })
            .collect()
    }
}

/// Errors produced when configuring or dispatching Telegram alerts.
///
/// Error messages are intentionally sanitized so the bot token is never exposed.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum TelegramAlertError {
    #[error("missing required environment variable: {0}")]
    MissingEnvVar(&'static str),
    #[error("invalid Telegram bot token: {0}")]
    InvalidBotToken(&'static str),
    #[error("invalid Telegram chat ID: {0}")]
    InvalidChatId(&'static str),
    #[error("invalid Telegram API base URL: {0}")]
    InvalidApiUrl(&'static str),
    #[error("failed to initialize Telegram HTTP client")]
    HttpClientInit,
    #[error("Telegram API returned HTTP {status}: {detail}")]
    HttpStatus { status: u16, detail: String },
    #[error("Telegram API rejected message: {detail}")]
    ApiRejected { detail: String },
    #[error("Telegram API request timed out")]
    Timeout,
    #[error("Telegram API request failed")]
    Transport,
    #[error("Telegram API delivery retries exhausted")]
    RetriesExhausted,
}

/// Configuration for [`TelegramAlertDispatcher`].
#[derive(Clone)]
pub struct TelegramConfig {
    bot_token: Zeroizing<String>,
    chat_id: String,
    api_base_url: Url,
    disable_notification: Option<bool>,
    message_thread_id: Option<i64>,
    extra_query_params: Vec<(String, String)>,
}

impl fmt::Debug for TelegramConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TelegramConfig")
            .field("bot_token", &"[REDACTED]")
            .field("chat_id", &self.chat_id)
            .field("api_base_url", &self.api_base_url.as_str())
            .field("disable_notification", &self.disable_notification)
            .field("message_thread_id", &self.message_thread_id)
            .field("extra_query_params_count", &self.extra_query_params.len())
            .finish()
    }
}

impl TelegramConfig {
    /// Creates a new validated configuration targeting the default Telegram Bot API host.
    pub fn new(
        bot_token: impl Into<String>,
        chat_id: impl Into<String>,
    ) -> Result<Self, TelegramAlertError> {
        let bot_token = validate_bot_token(bot_token.into())?;
        let chat_id = validate_chat_id(chat_id.into())?;
        let api_base_url = validate_api_base_url(TELEGRAM_API_BASE_URL)?;

        Ok(Self {
            bot_token,
            chat_id,
            api_base_url,
            disable_notification: None,
            message_thread_id: None,
            extra_query_params: Vec::new(),
        })
    }

    /// Reads `SENTINEL_TELEGRAM_BOT_TOKEN` and `SENTINEL_TELEGRAM_CHAT_ID` from the environment.
    pub fn from_env() -> Result<Self, TelegramAlertError> {
        Self::from_env_with(|key| env::var(key))
    }

    /// Reads credentials using a caller-supplied environment lookup function.
    pub fn from_env_with<F>(lookup: F) -> Result<Self, TelegramAlertError>
    where
        F: Fn(&str) -> Result<String, env::VarError>,
    {
        let bot_token = lookup(ENV_TELEGRAM_BOT_TOKEN)
            .map_err(|_| TelegramAlertError::MissingEnvVar(ENV_TELEGRAM_BOT_TOKEN))?;
        let chat_id = lookup(ENV_TELEGRAM_CHAT_ID)
            .map_err(|_| TelegramAlertError::MissingEnvVar(ENV_TELEGRAM_CHAT_ID))?;
        Self::new(bot_token, chat_id)
    }

    /// Overrides the Telegram Bot API base URL (HTTPS required except loopback hosts).
    pub fn with_api_base_url(mut self, base_url: &str) -> Result<Self, TelegramAlertError> {
        self.api_base_url = validate_api_base_url(base_url)?;
        Ok(self)
    }

    /// Sets the Telegram `disable_notification` query parameter.
    pub fn with_disable_notification(mut self, disable: bool) -> Self {
        self.disable_notification = Some(disable);
        self
    }

    /// Sets the Telegram `message_thread_id` query parameter for forum supergroup topics.
    pub fn with_message_thread_id(mut self, thread_id: i64) -> Self {
        self.message_thread_id = Some(thread_id);
        self
    }

    /// Appends an additional URL query parameter to outbound `sendMessage` requests.
    pub fn with_query_param(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.extra_query_params.push((key.into(), value.into()));
        self
    }

    /// Returns the configured target chat ID.
    pub fn chat_id(&self) -> &str {
        &self.chat_id
    }
}

/// JSON request body for Telegram Bot API `sendMessage`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TelegramSendMessagePayload {
    pub chat_id: String,
    pub text: String,
    pub parse_mode: &'static str,
    pub disable_web_page_preview: bool,
}

/// Outbound Telegram Bot API alert dispatcher.
pub struct TelegramAlertDispatcher {
    config: TelegramConfig,
    client: Client,
}

impl fmt::Debug for TelegramAlertDispatcher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TelegramAlertDispatcher")
            .field("config", &self.config)
            .finish()
    }
}

impl TelegramAlertDispatcher {
    /// Constructs a new dispatcher targeting `https://api.telegram.org`.
    pub fn new(
        bot_token: impl Into<String>,
        chat_id: impl Into<String>,
    ) -> Result<Self, TelegramAlertError> {
        Self::from_config(TelegramConfig::new(bot_token, chat_id)?)
    }

    /// Constructs a dispatcher from an existing validated [`TelegramConfig`].
    pub fn from_config(config: TelegramConfig) -> Result<Self, TelegramAlertError> {
        let client = Client::builder()
            .timeout(Duration::from_secs(DEFAULT_TIMEOUT_SECS))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| TelegramAlertError::HttpClientInit)?;
        Ok(Self { config, client })
    }

    /// Constructs a dispatcher from `SENTINEL_TELEGRAM_BOT_TOKEN` and `SENTINEL_TELEGRAM_CHAT_ID`.
    pub fn from_env() -> Result<Self, TelegramAlertError> {
        Self::from_config(TelegramConfig::from_env()?)
    }

    /// Constructs a dispatcher using a custom environment lookup function.
    pub fn from_env_with<F>(lookup: F) -> Result<Self, TelegramAlertError>
    where
        F: Fn(&str) -> Result<String, env::VarError>,
    {
        Self::from_config(TelegramConfig::from_env_with(lookup)?)
    }

    /// Overrides the Telegram Bot API base URL (e.g., for WireMock integration tests).
    pub fn with_api_base_url(mut self, base_url: &str) -> Result<Self, TelegramAlertError> {
        self.config = self.config.with_api_base_url(base_url)?;
        Ok(self)
    }

    /// Sets the `disable_notification` query parameter on outbound requests.
    pub fn with_disable_notification(mut self, disable: bool) -> Self {
        self.config = self.config.with_disable_notification(disable);
        self
    }

    /// Sets the `message_thread_id` query parameter on outbound requests.
    pub fn with_message_thread_id(mut self, thread_id: i64) -> Self {
        self.config = self.config.with_message_thread_id(thread_id);
        self
    }

    /// Appends a URL query parameter to outbound `sendMessage` requests.
    pub fn with_query_param(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.config = self.config.with_query_param(key, value);
        self
    }

    /// Builds the `sendMessage` endpoint URL while preserving base URL query parameters
    /// and appending any configured Telegram query parameters.
    pub fn build_endpoint_url(&self) -> Result<Url, TelegramAlertError> {
        let mut url = self.config.api_base_url.clone();
        let bot_segment = format!("bot{}", self.config.bot_token.as_str());
        url.path_segments_mut()
            .map_err(|_| TelegramAlertError::InvalidApiUrl("base URL cannot have path segments"))?
            .pop_if_empty()
            .extend([bot_segment.as_str(), "sendMessage"]);

        {
            let mut query_pairs = url.query_pairs_mut();
            if let Some(disable) = self.config.disable_notification {
                query_pairs.append_pair(
                    "disable_notification",
                    if disable { "true" } else { "false" },
                );
            }
            if let Some(thread_id) = self.config.message_thread_id {
                query_pairs.append_pair("message_thread_id", &thread_id.to_string());
            }
            for (key, value) in &self.config.extra_query_params {
                query_pairs.append_pair(key, value);
            }
        }

        // Remove empty trailing `?` if no query parameters were present or added.
        if url.query() == Some("") {
            url.set_query(None);
        }

        Ok(url)
    }

    /// Formats a Telegram `sendMessage` JSON payload for a [`SentinelEvent`].
    ///
    /// Returns `None` for non-breach events or breaches with [`RiskLevel::Ok`].
    pub fn build_payload(&self, event: &SentinelEvent) -> Option<TelegramSendMessagePayload> {
        let SentinelEvent::RiskThresholdBreached {
            contract_id,
            network,
            entry,
            remaining_ledgers,
            risk,
        } = event
        else {
            return None;
        };

        let badge = risk_badge_emoji(*risk)?;
        let text = format_html_alert(
            badge,
            *risk,
            contract_id,
            network,
            entry,
            *remaining_ledgers,
        );

        Some(TelegramSendMessagePayload {
            chat_id: self.config.chat_id.clone(),
            text,
            parse_mode: "HTML",
            disable_web_page_preview: true,
        })
    }

    /// Handles a single [`SentinelEvent`].
    ///
    /// Returns `Ok(true)` if an alert was dispatched, or `Ok(false)` if the event
    /// was ignored (not a risk breach or [`RiskLevel::Ok`]).
    pub async fn handle_event(&self, event: &SentinelEvent) -> Result<bool, TelegramAlertError> {
        let Some(payload) = self.build_payload(event) else {
            return Ok(false);
        };
        self.send_payload(&payload).await?;
        Ok(true)
    }

    /// Consumes [`SentinelEvent`] messages from a Tokio `mpsc` channel until the
    /// channel closes, dispatching alerts for every [`SentinelEvent::RiskThresholdBreached`].
    ///
    /// Returns any delivery errors encountered while processing the stream.
    pub async fn run(&self, mut rx: mpsc::Receiver<SentinelEvent>) -> Vec<TelegramAlertError> {
        let mut errors = Vec::new();
        while let Some(event) = rx.recv().await {
            if let Err(error) = self.handle_event(&event).await {
                errors.push(error);
            }
        }
        errors
    }

    async fn send_payload(
        &self,
        payload: &TelegramSendMessagePayload,
    ) -> Result<(), TelegramAlertError> {
        let url = self.build_endpoint_url()?;
        let request = self.client.post(url).json(payload);

        for attempt in 0..MAX_DISPATCH_ATTEMPTS {
            let response = request
                .try_clone()
                .ok_or(TelegramAlertError::Transport)?
                .send()
                .await;

            let retry_delay = match response {
                Ok(resp) if resp.status().is_success() => {
                    let body = resp.text().await.unwrap_or_default();
                    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&body) {
                        if parsed.get("ok").and_then(serde_json::Value::as_bool) == Some(false) {
                            let detail = sanitize_detail(
                                parsed
                                    .get("description")
                                    .and_then(serde_json::Value::as_str)
                                    .unwrap_or("request rejected by Telegram API"),
                                self.config.bot_token.as_str(),
                            );
                            return Err(TelegramAlertError::ApiRejected { detail });
                        }
                    }
                    return Ok(());
                }
                Ok(resp) => {
                    let status = resp.status();
                    let retry_after = resp
                        .headers()
                        .get(reqwest::header::RETRY_AFTER)
                        .and_then(|value| value.to_str().ok())
                        .and_then(|value| value.parse::<u64>().ok())
                        .map(|seconds| Duration::from_secs(seconds.min(5)));

                    let is_retryable = status.is_server_error() || status.as_u16() == 429;
                    if attempt + 1 == MAX_DISPATCH_ATTEMPTS || !is_retryable {
                        let body = resp.text().await.unwrap_or_default();
                        let raw_detail = serde_json::from_str::<serde_json::Value>(&body)
                            .ok()
                            .and_then(|json| {
                                json.get("description")
                                    .and_then(serde_json::Value::as_str)
                                    .map(str::to_owned)
                            })
                            .unwrap_or_else(|| {
                                status
                                    .canonical_reason()
                                    .unwrap_or("unexpected HTTP error")
                                    .to_owned()
                            });
                        return Err(TelegramAlertError::HttpStatus {
                            status: status.as_u16(),
                            detail: sanitize_detail(&raw_detail, self.config.bot_token.as_str()),
                        });
                    }
                    retry_after
                }
                Err(err)
                    if attempt + 1 < MAX_DISPATCH_ATTEMPTS
                        && (err.is_timeout() || err.is_connect()) =>
                {
                    None
                }
                Err(err) if err.is_timeout() => return Err(TelegramAlertError::Timeout),
                Err(_) => return Err(TelegramAlertError::Transport),
            };

            tokio::time::sleep(retry_delay.unwrap_or(Duration::from_millis(100 << attempt))).await;
        }

        Err(TelegramAlertError::RetriesExhausted)
    }
}

/// Escapes HTML special characters required by Telegram Bot API HTML parse mode.
pub fn escape_html(input: &str) -> String {
    let mut escaped = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            _ => escaped.push(ch),
        }
    }
    escaped
}

/// Maps a [`RiskLevel`] to its Telegram risk badge emoji.
pub fn risk_badge_emoji(risk: RiskLevel) -> Option<&'static str> {
    match risk {
        RiskLevel::Ok => None,
        RiskLevel::Warn => Some("⚠️"),
        RiskLevel::Critical | RiskLevel::Archived => Some("🚨"),
    }
}

/// Builds a direct Stellar Expert explorer URL for a Soroban contract.
pub fn stellar_expert_contract_url(network: &str, contract_id: &str) -> String {
    let explorer_network = match network.trim().to_ascii_lowercase().as_str() {
        "mainnet" | "public" => "public",
        _ => "testnet",
    };

    let mut url =
        Url::parse("https://stellar.expert/explorer/").expect("valid Stellar Expert base URL");
    url.path_segments_mut()
        .expect("Stellar Expert URL supports path segments")
        .pop_if_empty()
        .extend([explorer_network, "contract", contract_id]);
    url.to_string()
}

fn format_html_alert(
    badge: &str,
    risk: RiskLevel,
    contract_id: &str,
    network: &str,
    entry: &StorageEntry,
    remaining_ledgers: i64,
) -> String {
    let escaped_contract = escape_html(contract_id);
    let escaped_network = escape_html(network);
    let truncated_key = truncate_chars(entry.key_identifier(), MAX_KEY_DISPLAY_CHARS);
    let escaped_key = escape_html(&truncated_key);
    let explorer_url = escape_html(&stellar_expert_contract_url(network, contract_id));

    format!(
        "{badge} <b>Soroban Sentinel Alert — {risk}</b>\n\n\
         <b>Contract:</b> <code>{escaped_contract}</code>\n\
         <b>Network:</b> {escaped_network}\n\
         <b>Entry:</b> <code>{escaped_key}</code> ({durability})\n\
         <b>Live-Until Ledger:</b> <code>{live_until}</code> ({remaining_ledgers} ledgers remaining)\n\
         <b>Explorer:</b> <a href=\"{explorer_url}\">View on Stellar Expert</a>",
        durability = entry.durability(),
        live_until = entry.live_until_ledger(),
    )
}

fn truncate_chars(input: &str, max_chars: usize) -> String {
    let mut chars = input.chars();
    let prefix: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{prefix}…")
    } else {
        prefix
    }
}

fn validate_bot_token(raw: String) -> Result<Zeroizing<String>, TelegramAlertError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(TelegramAlertError::InvalidBotToken(
            "token must not be empty",
        ));
    }
    if trimmed
        .chars()
        .any(|ch| ch.is_whitespace() || ch.is_control() || matches!(ch, '/' | '?' | '#' | '\\'))
    {
        return Err(TelegramAlertError::InvalidBotToken(
            "token contains disallowed URL or control characters",
        ));
    }
    Ok(Zeroizing::new(trimmed.to_owned()))
}

fn validate_chat_id(raw: String) -> Result<String, TelegramAlertError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(TelegramAlertError::InvalidChatId(
            "chat ID must not be empty",
        ));
    }
    if trimmed.chars().any(char::is_control) {
        return Err(TelegramAlertError::InvalidChatId(
            "chat ID must not contain control characters",
        ));
    }
    Ok(trimmed.to_owned())
}

fn validate_api_base_url(raw: &str) -> Result<Url, TelegramAlertError> {
    let parsed = Url::parse(raw)
        .map_err(|_| TelegramAlertError::InvalidApiUrl("must be a valid HTTP(S) URL"))?;
    let is_loopback = parsed.host_str().is_some_and(|host| {
        host.eq_ignore_ascii_case("localhost") || host == "127.0.0.1" || host == "::1"
    });
    if parsed.scheme() != "https" && !(parsed.scheme() == "http" && is_loopback) {
        return Err(TelegramAlertError::InvalidApiUrl(
            "must use HTTPS, except loopback test endpoints",
        ));
    }
    if parsed.cannot_be_a_base() {
        return Err(TelegramAlertError::InvalidApiUrl(
            "base URL cannot have path segments",
        ));
    }
    Ok(parsed)
}

fn sanitize_detail(detail: &str, bot_token: &str) -> String {
    if bot_token.is_empty() {
        detail.to_owned()
    } else {
        detail.replace(bot_token, "[REDACTED]")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sentinel_core::{Durability, EntryType};

    #[test]
    fn escape_html_escapes_special_characters() {
        assert_eq!(
            escape_html("<script>alert(\"x & y\")</script>"),
            "&lt;script&gt;alert(&quot;x &amp; y&quot;)&lt;/script&gt;"
        );
    }

    #[test]
    fn risk_badge_maps_warn_and_critical_emojis() {
        assert_eq!(risk_badge_emoji(RiskLevel::Ok), None);
        assert_eq!(risk_badge_emoji(RiskLevel::Warn), Some("⚠️"));
        assert_eq!(risk_badge_emoji(RiskLevel::Critical), Some("🚨"));
        assert_eq!(risk_badge_emoji(RiskLevel::Archived), Some("🚨"));
    }

    #[test]
    fn stellar_expert_url_maps_networks_and_encodes_contract_id() {
        let contract = "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM";
        assert_eq!(
            stellar_expert_contract_url("testnet", contract),
            format!("https://stellar.expert/explorer/testnet/contract/{contract}")
        );
        assert_eq!(
            stellar_expert_contract_url("mainnet", contract),
            format!("https://stellar.expert/explorer/public/contract/{contract}")
        );
        assert_eq!(
            stellar_expert_contract_url("PUBLIC", contract),
            format!("https://stellar.expert/explorer/public/contract/{contract}")
        );
        let injected = stellar_expert_contract_url("testnet", "CAAA<test>?foo=bar");
        assert!(!injected.contains('<'));
        assert!(!injected.contains("?foo=bar"));
    }

    #[test]
    fn debug_output_redacts_bot_token() {
        let dispatcher = TelegramAlertDispatcher::new("123456:super-secret-token", "-100999")
            .expect("valid dispatcher");
        let debug = format!("{dispatcher:?}");
        assert!(!debug.contains("super-secret-token"));
        assert!(debug.contains("[REDACTED]"));
    }

    #[test]
    fn rejects_invalid_tokens_and_insecure_urls() {
        assert!(matches!(
            TelegramConfig::new("123456:bad?chat_id=evil", "-1001"),
            Err(TelegramAlertError::InvalidBotToken(_))
        ));
        assert!(matches!(
            TelegramConfig::new("123456:bad/path", "-1001"),
            Err(TelegramAlertError::InvalidBotToken(_))
        ));
        assert!(matches!(
            TelegramConfig::new("123456:valid", "-1001")
                .unwrap()
                .with_api_base_url("http://api.telegram.org"),
            Err(TelegramAlertError::InvalidApiUrl(_))
        ));
    }

    #[test]
    fn build_payload_escapes_html_in_contract_and_symbol() {
        let dispatcher = TelegramAlertDispatcher::new("123456:test-token", "-10042").unwrap();
        let entry = StorageEntry::new(
            "persistent:<ADMIN & OPS>",
            EntryType::PersistentData,
            Durability::Persistent,
            128,
            5_500_000,
            5_400_000,
        );
        let event = SentinelEvent::risk_threshold_breached(
            "CAAA<TEST>&1",
            "testnet",
            entry,
            100,
            RiskLevel::Warn,
        );
        let payload = dispatcher.build_payload(&event).expect("expected payload");
        assert_eq!(payload.parse_mode, "HTML");
        assert_eq!(payload.chat_id, "-10042");
        assert!(payload.text.contains("⚠️"));
        assert!(payload.text.contains("CAAA&lt;TEST&gt;&amp;1"));
        assert!(payload.text.contains("persistent:&lt;ADMIN &amp; OPS&gt;"));
        assert!(payload.text.contains("5500000"));
        assert!(!payload.text.contains("<ADMIN"));
        assert!(!payload.text.contains("<TEST>"));
    }
}
