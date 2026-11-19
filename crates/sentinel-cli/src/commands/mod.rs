//! CLI command execution modules.

pub mod diff_networks;
pub mod scan;

pub use diff_networks::run_diff_networks;
pub use scan::{
    resolve_rpc_url, run_scan, EXIT_CRITICAL_OR_ARCHIVED, EXIT_INVALID_ARGS, EXIT_OK,
    EXIT_RPC_FAILURE, EXIT_WARN,
};
