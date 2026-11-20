//! Command-line interface library for Soroban Sentinel.

#![forbid(unsafe_code)]

pub mod args;
pub mod commands;
pub mod config;
pub mod formatters;

pub use args::{BumpArgs, BundleArgs, Cli, Commands, CostMatrixArgs, NetworkArgs, ScanArgs};
pub use commands::{
    resolve_rpc_url, run_bump, run_bundle, run_cost_matrix, run_network, run_scan,
    EXIT_CRITICAL_OR_ARCHIVED, EXIT_INVALID_ARGS, EXIT_OK, EXIT_RPC_FAILURE, EXIT_WARN,
};
pub use formatters::{render_json, render_text, JsonEntryReport, JsonSummary, JsonTTLReport};
