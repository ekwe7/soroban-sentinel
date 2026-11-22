
// CLI command execution modules.

pub mod bundle;
pub mod check;
pub mod cost_matrix;
pub mod diff;
pub mod forecast;
pub mod init;
pub mod keys;
pub mod restore;
pub mod scan;
pub mod service;
#[cfg(feature = "tui")]
pub mod tui;

pub use bundle::run_bundle;
pub use check::run_check;
pub use cost_matrix::run_cost_matrix;
pub use diff::run_diff_reports;
pub use forecast::run_forecast;
pub use init::run_init;
pub use keys::{run_key_decode, run_key_encode};
pub use restore::run_restore;

pub use scan::{
    resolve_rpc_url,
    run_scan,
    EXIT_CRITICAL_OR_ARCHIVED,
    EXIT_INVALID_ARGS,
    EXIT_OK,
    EXIT_RPC_FAILURE,
    EXIT_WARN,
    resolve_rpc_url, resolve_threshold_config, run_scan, EXIT_CRITICAL_OR_ARCHIVED,
    EXIT_INVALID_ARGS, EXIT_OK, EXIT_RPC_FAILURE, EXIT_WARN,
};
pub use service::run_install_service;
