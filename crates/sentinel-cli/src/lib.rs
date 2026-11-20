//! Command-line interface library for Soroban Sentinel.

#![forbid(unsafe_code)]

pub mod args;
pub mod commands;
pub mod config;
pub mod formatters;
pub mod telemetry;

pub use args::{BundleArgs, Cli, Commands, ScanArgs};
pub use commands::{
    generate_schema, lint_manifest, resolve_rpc_url, run_bundle, run_scan, schema_command,
    EXIT_CRITICAL_OR_ARCHIVED, EXIT_INVALID_ARGS, EXIT_OK, EXIT_RPC_FAILURE, EXIT_WARN,
};
pub use formatters::{
    render_csv, render_json, render_text, JsonEntryReport, JsonSummary, JsonTTLReport, CSV_HEADER,
};
