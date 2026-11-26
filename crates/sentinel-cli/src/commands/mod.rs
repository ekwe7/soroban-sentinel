// CLI command execution modules.

pub mod audit_keys;
pub mod benchmark_rpc;
pub mod bump;
pub mod bundle;
pub mod check;
pub mod completions;
pub mod cost_matrix;
pub mod diff;
pub mod doctor;
pub mod diff_networks;
pub mod dry_run;
pub mod estimate_gas;
pub mod explain;
pub mod export_keys;
pub mod forecast;
pub mod generate_docker;
pub mod init;
pub mod key_diff;
pub mod keys;
pub mod network;
pub mod redact;
pub mod restore;
pub mod scan;
pub mod schema;
pub mod service;
pub mod trace_route;
#[cfg(feature = "tui")]
pub mod tui;
pub mod watch;

pub use audit_keys::run_audit_keys;
pub use benchmark_rpc::{run_benchmark, EXIT_BENCHMARK_ERROR, EXIT_BENCHMARK_OK};
pub use bump::run_bump;
pub use bundle::run_bundle;
pub use check::run_check;
pub use completions::run_completions;
pub use cost_matrix::run_cost_matrix;
pub use diff::run_diff_reports;
pub use diff_networks::run_diff_networks;
pub use dry_run::run_dry_run;
pub use estimate_gas::run_estimate_gas;
pub use explain::run_explain;
pub use export_keys::run_export_keys;
pub use forecast::run_forecast;
pub use generate_docker::run_generate_docker;
pub use init::run_init;
pub use key_diff::run_key_diff;
pub use keys::{run_key_decode, run_key_encode};
pub use network::run_network;
pub use redact::run_redact;
pub use restore::run_restore;

pub use scan::{
    resolve_rpc_url, resolve_threshold_config, run_scan, EXIT_CRITICAL_OR_ARCHIVED,
    EXIT_INVALID_ARGS, EXIT_OK, EXIT_RPC_FAILURE, EXIT_WARN,
};
pub use schema::{generate_schema, lint_manifest, schema_command};
pub use service::run_install_service;

pub mod export_openapi;
pub mod health_matrix;
pub use export_openapi::run_export_openapi;
pub use health_matrix::run_health_matrix;
pub mod top_consumers;
pub use top_consumers::run_top_consumers;
pub mod trend;
pub use trend::run_trend;
