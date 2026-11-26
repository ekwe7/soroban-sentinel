//! Command-line interface library for Soroban Sentinel.

#![forbid(unsafe_code)]

pub mod alerts;
pub mod args;
pub mod commands;
pub mod config;
pub mod formatters;

pub use alerts::{compute_hmac_sha256, format_timestamp, send_webhook_alert, WebhookSigner, send_alert_with_dedup, EmailPayload, EmailConfig, RiskDisplay, DeduplicationTracker, send_email};
pub use args::{BundleArgs, Cli, Commands, ScanArgs};
pub use commands::{
    resolve_rpc_url, run_bundle, run_scan, EXIT_CRITICAL_OR_ARCHIVED, EXIT_INVALID_ARGS, EXIT_OK,
    EXIT_RPC_FAILURE, EXIT_WARN,
};
pub use formatters::{render_json, render_text, JsonEntryReport, JsonSummary, JsonTTLReport};
