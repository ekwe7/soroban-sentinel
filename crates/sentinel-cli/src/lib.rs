//! Command-line interface library for Soroban Sentinel.

#![forbid(unsafe_code)]

pub mod args;
pub mod commands;
pub mod config;
pub mod formatters;

pub use args::{BundleArgs, Cli, Commands, OutputFormat, ScanArgs};
pub use commands::{
    resolve_rpc_url, run_bundle, run_scan, EXIT_CRITICAL_OR_ARCHIVED, EXIT_INVALID_ARGS, EXIT_OK,
    EXIT_RPC_FAILURE, EXIT_WARN,
};
pub use formatters::{
    render_json, render_markdown, render_prometheus, render_text, JsonEntryReport, JsonSummary,
    JsonTTLReport,
};
