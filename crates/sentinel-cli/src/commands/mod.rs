//! CLI command execution modules.

pub mod audit_keys;
pub mod bump;
pub mod bundle;
pub mod check;
pub mod cost_matrix;
pub mod diff;
pub mod diff_networks;
pub mod dry_run;
pub mod forecast;
pub mod init;
pub mod keys;
pub mod network;
pub mod redact;
pub mod restore;
pub mod scan;
pub mod service;
#[cfg(feature = "tui")]
pub mod tui;

pub use audit_keys::run_audit_keys;
pub use bump::run_bump;
pub use bundle::run_bundle;
pub use check::run_check;
pub use cost_matrix::run_cost_matrix;
pub use diff::run_diff_reports;
pub use diff_networks::run_diff_networks;
pub use dry_run::run_dry_run;
pub use forecast::run_forecast;
pub use init::run_init;
pub use keys::{run_key_decode, run_key_encode};
pub use network::run_network;
pub use redact::run_redact;
pub use restore::run_restore;

pub use scan::{
    resolve_rpc_url, resolve_threshold_config, run_scan, EXIT_CRITICAL_OR_ARCHIVED,
    EXIT_INVALID_ARGS, EXIT_OK, EXIT_RPC_FAILURE, EXIT_WARN,
};
pub mod doctor;
pub mod watch;
pub use service::run_install_service;

pub mod top_consumers;
pub use top_consumers::run_top_consumers;
