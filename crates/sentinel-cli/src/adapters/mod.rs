//! Outbound infrastructure adapters for `sentinel-cli`.

pub mod alert_teams;
pub mod alert_telegram;

pub use alert_telegram::{
    escape_html, risk_badge_emoji, stellar_expert_contract_url, SentinelEvent,
    TelegramAlertDispatcher, TelegramAlertError, TelegramConfig, TelegramSendMessagePayload,
    ENV_TELEGRAM_BOT_TOKEN, ENV_TELEGRAM_CHAT_ID, TELEGRAM_API_BASE_URL,
};
