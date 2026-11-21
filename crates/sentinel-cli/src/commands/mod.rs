//! CLI command execution modules.

pub mod benchmark_rpc;
pub mod bundle;
pub mod check;
pub mod cost_matrix;
pub mod restore;
pub mod scan;
#[cfg(feature = "tui")]
pub mod tui;

pub use benchmark_rpc::{run_benchmark, EXIT_BENCHMARK_ERROR, EXIT_BENCHMARK_OK};
pub use bundle::run_bundle;
pub use check::run_check;
pub use cost_matrix::run_cost_matrix;
pub use restore::run_restore;

pub use scan::{
    resolve_rpc_url, run_scan, EXIT_CRITICAL_OR_ARCHIVED, EXIT_INVALID_ARGS, EXIT_OK,
    EXIT_RPC_FAILURE, EXIT_WARN,
};
