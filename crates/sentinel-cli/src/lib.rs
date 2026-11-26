//! Command-line interface library for Soroban Sentinel.

#![forbid(unsafe_code)]

pub mod adapters;
pub mod alerts;
pub mod args;
pub mod commands;
pub mod config;
pub mod formatters;
pub mod telemetry;
#[cfg(feature = "tui")]
pub mod tui;

pub use alerts::{EmailPayload, RiskDisplay, WebhookSigner};
pub use args::*;
pub use commands::*;
#[cfg(feature = "parquet")]
pub use formatters::render_parquet;
pub use formatters::*;
