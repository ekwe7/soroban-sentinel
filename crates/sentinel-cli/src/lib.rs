//! Command-line interface library for Soroban Sentinel.

#![forbid(unsafe_code)]

pub mod args;
pub mod commands;
pub mod config;
pub mod formatters;
pub mod telemetry;
#[cfg(feature = "tui")]
pub mod tui;

pub use args::{
    BundleArgs, Cli, Commands, DiffReportsArgs, ForecastArgs, InitArgs, InstallServiceArgs,
    KeyDecodeArgs, KeyEncodeArgs, RestoreArgs, ScanArgs,
};
pub use commands::{
    resolve_rpc_url, run_bundle, run_diff_reports, run_init, run_install_service, run_key_decode,
    run_key_encode, run_restore, run_scan, EXIT_CRITICAL_OR_ARCHIVED, EXIT_INVALID_ARGS, EXIT_OK,
    EXIT_RPC_FAILURE, EXIT_WARN,
};
#[cfg(feature = "parquet")]
pub use formatters::render_parquet;
pub use formatters::{
    build_tabular_rows, render_csv, render_json, render_text, JsonEntryReport, JsonSummary,
    JsonTTLReport, TabularRow, CSV_HEADER,
};
