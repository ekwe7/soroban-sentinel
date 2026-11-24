
use clap::{Args, Parser, Subcommand};
use sentinel_core::domain::DEFAULT_SAMPLE_WINDOW_LEDGERS;

/// Autonomous storage TTL monitoring and expiry-risk detection for Soroban smart contracts.
#[derive(Parser, Debug)]
#[command(
    name = "sentinel",
    author = "Soroban Sentinel Contributors",
    version,
    about = "Autonomous storage TTL monitoring and expiry-risk detection for Soroban smart contracts on Stellar",
    long_about = "Soroban Sentinel provides operator-grade storage TTL monitoring, expiry-risk classification, and automated maintenance detection for smart contracts on the Stellar network."
)]
pub struct Cli {
    /// OTLP collector endpoint for traces and metrics (e.g. "http://localhost:4317").
    ///
    /// Falls back to the OTEL_EXPORTER_OTLP_ENDPOINT environment variable.
    #[arg(long = "otlp-endpoint", global = true, value_name = "URL")]
    pub otlp_endpoint: Option<String>,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Scan a contract and inspect storage entry TTL status and expiry risk
    Scan(Box<ScanArgs>),
    /// Rank resolved contract entries by serialized byte size
    TopConsumers(TopConsumersArgs),
    /// Check Soroban RPC connectivity, network identity, and protocol support
    Doctor(DoctorArgs),
    /// Monitor one contract and notify configured incident providers
    Watch(Box<WatchArgs>),

    /// Compare the cost of extending contract storage TTL across duration tiers
    CostMatrix(CostMatrixArgs),

    /// Bundle contract and persistent-key TTL extensions into simulated transactions
    Bundle(BundleArgs),

    /// Build, sign, and optionally submit a TTL extension transaction
    Bump(BumpArgs),

    /// Detect archived persistent keys and assemble CAP-0046 restoration transactions
    Restore(RestoreArgs),

    /// Check one contract as a monitoring plugin, or scan a fleet manifest
    Check(CheckArgs),

    /// Audit contract storage placement, entry sizes, and cost-saving opportunities
    AuditKeys(AuditKeysArgs),
    /// Generate a systemd or launchd service definition for autonomous watching
    InstallService(InstallServiceArgs),

    /// Generate a sentinel.toml fleet manifest
    Init(InitArgs),

    /// Encode a Soroban storage key expression as XDR
    KeyEncode(KeyEncodeArgs),

    /// Decode a base64 Soroban storage key XDR value
    KeyDecode(KeyDecodeArgs),

    /// Compare two saved JSON TTL reports without using RPC
    DiffReports(DiffReportsArgs),

    /// Compare a contract's storage health across two Stellar networks
    DiffNetworks(DiffNetworksArgs),

    /// Estimate the fee for extending a contract footprint TTL
    Forecast(ForecastArgs),

    /// Display network state-archival and contract entry limits
    Network(NetworkArgs),
}

#[derive(Args, Debug)]
pub struct DoctorArgs {
    #[arg(long, value_name = "URL", env = "SOROBAN_RPC_URL")]
    pub rpc_url: Option<String>,
    #[arg(long, default_value = "testnet", value_name = "NETWORK")]
    pub network: String,
    /// Disable ANSI color codes in text output (also honored via the NO_COLOR environment variable)
    #[arg(long = "no-color")]
    pub no_color: bool,
}

// Avoid Debug implementations that print provider credentials.
#[derive(Args)]
pub struct WatchArgs {
    #[command(flatten)]
    pub scan: ScanArgs,
    /// Seconds between scans
    #[arg(long, default_value_t = 60, value_parser = clap::value_parser!(u64).range(1..))]
    pub interval: u64,
    #[arg(long, env = "PAGERDUTY_ROUTING_KEY", hide_env_values = true)]
    pub pagerduty_routing_key: Option<String>,
    #[arg(long, env = "OPSGENIE_API_KEY", hide_env_values = true)]
    pub opsgenie_api_key: Option<String>,
    /// Datadog API key for metrics and events submission
    #[arg(
        long,
        env = "DATADOG_API_KEY",
        hide_env_values = true,
        value_name = "KEY"
    )]
    pub datadog_api_key: Option<String>,
    /// Datadog intake site: "datadoghq.com" (US, default) or "datadoghq.eu" (EU)
    #[arg(long, default_value = "datadoghq.com", value_name = "SITE",
          value_parser = ["datadoghq.com", "datadoghq.eu"])]
    pub datadog_site: String,
}

impl std::fmt::Debug for WatchArgs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WatchArgs").finish_non_exhaustive()
    }
}

/// Command-line arguments for forecast.
#[derive(Args, Debug, Clone)]
pub struct ForecastArgs {
    #[arg(value_name = "CONTRACT_ID")]
    pub contract_id: String,
    #[arg(long, value_name = "LEDGER", value_parser = clap::value_parser!(u32).range(1..))]
    pub extend_to: u32,
    #[arg(long, value_name = "G...")]
    pub source: String,
    #[arg(long, default_value = "testnet")]
    pub network: String,
    #[arg(long, env = "SOROBAN_RPC_URL")]
    pub rpc_url: Option<String>,
    #[arg(long, default_value = "text", value_parser = ["text", "json"])]
    pub format: String,
    #[arg(long = "key", value_name = "SYMBOL")]
    pub key: Vec<String>,
    #[arg(long = "key-xdr", value_name = "BASE64_XDR")]
    pub key_xdr: Vec<String>,
    #[arg(long = "key-expr", value_name = "EXPR")]
    pub key_expr: Vec<String>,
}

/// Command-line arguments for `sentinel init`.
#[derive(Args, Debug, Clone)]
pub struct InitArgs {
    /// Path to the manifest to create
    #[arg(long, default_value = "sentinel.toml", value_name = "PATH")]
    pub output: std::path::PathBuf,

    /// Target Stellar network
    #[arg(long, default_value = "testnet", value_name = "NETWORK")]
    pub network: String,

    /// Contract ID to include (repeatable)
    #[arg(long = "contract", value_name = "ID")]
    pub contracts: Vec<String>,

    /// Persistent symbol key to include on every contract (repeatable)
    #[arg(long = "key", value_name = "SYM")]
    pub keys: Vec<String>,

    /// Warning threshold in ledgers
    #[arg(long, default_value_t = 120960, value_name = "LEDGERS")]
    pub warning_threshold: u32,

    /// Critical threshold in ledgers
    #[arg(long, value_name = "LEDGERS")]
    pub critical_threshold: Option<u32>,

    /// Do not prompt for missing values
    #[arg(long)]
    pub non_interactive: bool,

    /// Replace an existing output file
    #[arg(long)]
    pub force: bool,
}

/// Command-line arguments for `sentinel key-encode`.
#[derive(Args, Debug, Clone)]
pub struct KeyEncodeArgs {
    /// Key expression, such as `sym:Balance` or `vec[sym:Balance, u32:1]`
    #[arg(value_name = "EXPRESSION")]
    pub expression: String,

    /// Contract ID; when supplied, encode a full LedgerKey instead of ScVal
    #[arg(long, value_name = "ID")]
    pub contract: Option<String>,

    /// Ledger data durability for full LedgerKey output
    #[arg(long, default_value = "persistent", value_parser = ["persistent", "temporary"])]
    pub durability: String,

    /// Output format
    #[arg(long, default_value = "text", value_parser = ["text", "json"])]
    pub format: String,
}

/// Command-line arguments for `sentinel key-decode`.
#[derive(Args, Debug, Clone)]
pub struct KeyDecodeArgs {
    /// Base64 XDR for a ScVal or LedgerKey
    #[arg(value_name = "BASE64_XDR")]
    pub xdr: String,

    /// Output format
    #[arg(long, default_value = "text", value_parser = ["text", "json"])]
    pub format: String,
}

/// Command-line arguments for `sentinel diff-reports`.
#[derive(Args, Debug, Clone)]
pub struct DiffReportsArgs {
    /// Older JSON TTL report
    #[arg(value_name = "OLD_JSON")]
    pub old: std::path::PathBuf,

    /// Newer JSON TTL report
    #[arg(value_name = "NEW_JSON")]
    pub new: std::path::PathBuf,

    /// Output format
    #[arg(long, default_value = "text", value_parser = ["text", "json"])]
    pub format: String,

    /// Return exit code 2 when a risk regression or removal is found
    #[arg(long)]
    pub fail_on_regression: bool,
}

/// Command-line arguments for `sentinel diff-networks`.
#[derive(Args, Debug, Clone)]
pub struct DiffNetworksArgs {
    /// Contract address to scan on both networks
    #[arg(value_name = "CONTRACT_ID")]
    pub contract_id: String,

    /// First Stellar network
    #[arg(long = "network-a", value_name = "NET_A")]
    pub network_a: String,

    /// Second Stellar network
    #[arg(long = "network-b", value_name = "NET_B")]
    pub network_b: String,

    /// Custom Soroban RPC endpoint for network A
    #[arg(long = "rpc-url-a", value_name = "URL")]
    pub rpc_url_a: Option<String>,

    /// Custom Soroban RPC endpoint for network B
    #[arg(long = "rpc-url-b", value_name = "URL")]
    pub rpc_url_b: Option<String>,

    /// TTL horizon difference in ledgers that is considered significant
    #[arg(
        long = "ttl-drift-threshold",
        default_value_t = 0,
        value_name = "LEDGERS"
    )]
    pub ttl_drift_threshold: u32,

    /// Persistent storage symbol key to compare (repeatable)
    #[arg(long = "key", value_name = "SYMBOL")]
    pub key: Vec<String>,

    /// Persistent storage key encoded as base64 XDR (repeatable)
    #[arg(long = "key-xdr", value_name = "BASE64_XDR")]
    pub key_xdr: Vec<String>,

    /// Persistent storage key encoded as a typed expression (repeatable)
    #[arg(long = "key-expr", value_name = "EXPR")]
    pub key_expr: Vec<String>,

    /// Output format
    #[arg(long, default_value = "text", value_parser = ["text", "json"])]
    pub format: String,
}

/// Command-line arguments for the `sentinel scan` command.
#[derive(Args, Debug, Clone)]
pub struct ScanArgs {
    /// Target Soroban smart contract address (C... Strkey format) or a contract alias
    #[arg(value_name = "CONTRACT_ID")]
    pub contract_id: String,

    /// Custom Soroban RPC endpoint URL
    #[arg(long, value_name = "URL", env = "SOROBAN_RPC_URL")]
    pub rpc_url: Option<String>,

    /// Target Stellar network
    #[arg(long, default_value = "testnet", value_name = "NETWORK")]
    pub network: String,

    /// Output format: text, json, or sarif
    #[arg(long, default_value = "text", value_name = "FORMAT")]
    pub format: String,

    /// Persistent storage key symbol to inspect (repeatable)

    /// Output format for scan results: "text" (colorized ASCII table), "json" (machine-readable), "csv" (flat tabular export), or "sarif" (GitHub Code Scanning)

    /// Output format for scan results: "text" (colorized ASCII table), "json" (machine-readable), "csv" (flat tabular export), "parquet" (binary columnar export, requires the `parquet` Cargo feature), or "sarif" (GitHub Code Scanning)
    /// Output format for scan results: "text" (colorized ASCII table), "json" (machine-readable), "ndjson" (line-delimited JSON per storage entry), "csv" (flat tabular export), "parquet" (binary columnar export, requires the `parquet` Cargo feature), "sarif" (GitHub Code Scanning), or "markdown"
    #[arg(long, default_value = "text", value_name = "FORMAT")]
    pub format: String,

    /// Disable ANSI color codes in text output (also honored via the NO_COLOR environment variable)
    #[arg(long = "no-color")]
    pub no_color: bool,

    /// Write report output to a file instead of stdout (existing files are overwritten)
    #[arg(long, value_name = "PATH")]
    pub output: Option<String>,

    /// Persistent storage key symbol to inspect (can be specified multiple times)
    #[arg(long = "key", value_name = "SYMBOL")]
    pub key: Vec<String>,

    /// Persistent storage key encoded as raw base64 ScVal XDR (repeatable)
    #[arg(long = "key-xdr", value_name = "BASE64_XDR")]
    pub key_xdr: Vec<String>,

    /// Persistent storage key encoded as typed expression (repeatable)
    #[arg(long = "key-expr", value_name = "EXPR")]
    pub key_expr: Vec<String>,

    /// Warning threshold in ledgers
    #[arg(long, default_value_t = 120960, value_name = "LEDGERS")]
    pub threshold: u32,

    /// Critical threshold in ledgers
    #[arg(long, value_name = "LEDGERS")]
    pub critical_threshold: Option<u32>,

    /// Custom RPC request timeout in seconds
    #[arg(long = "rpc-timeout", value_name = "SECS")]
    pub rpc_timeout: Option<u64>,

    /// Maximum RPC retry attempts
    #[arg(long = "rpc-max-retries", value_name = "N")]
    pub rpc_max_retries: Option<u32>,

    /// Override ledger close velocity in seconds per ledger
    #[arg(long = "ledger-seconds", value_name = "SECS")]
    pub ledger_seconds: Option<f64>,

    /// Path to a previous JSON scan report baseline file
    /// Number of recent ledgers sampled to measure storage burn velocity (2–10000)
    #[arg(
        long = "sample-window",
        default_value_t = DEFAULT_SAMPLE_WINDOW_LEDGERS,
        value_name = "N_LEDGERS"
    )]
    pub sample_window: u32,

    /// Path to a previous JSON scan report baseline file for comparison
    #[arg(long = "baseline", value_name = "PATH")]
    pub baseline: Option<String>,

    /// Return exit code 2 if any entry regressed compared to baseline
    #[arg(long = "fail-on-regression")]
    pub fail_on_regression: bool,

    /// Launch the interactive TUI dashboard (requires the `tui` Cargo feature and a TTY)
    #[arg(long)]
    pub tui: bool,

    /// Webhook URL for WARN/CRITICAL alerts
    #[arg(
        long = "alert-webhook",
        env = "SENTINEL_ALERT_WEBHOOK",
        value_name = "URL"
    )]
    pub alert_webhook: Option<String>,
    /// Alert payload format
    #[arg(long = "alert-kind", default_value = "slack", value_parser = ["slack", "discord"])]
    pub alert_kind: String,
    /// Minimum risk level that triggers an alert
    #[arg(long = "alert-min-level", default_value = "warn", value_parser = ["warn", "critical"])]
    pub alert_min_level: String,

    /// Replay an offline .sentinel-snap ledger snapshot instead of querying RPC
    #[arg(long = "snapshot", value_name = "PATH")]
    pub snapshot: Option<String>,

    /// Render actionable TTL extension recommendations.
    #[arg(long)]
    pub recommend: bool,

    /// Target maintenance horizon in days used by recommendations.
    #[arg(long, default_value_t = 30, value_name = "DAYS")]
    pub target_days: u32,
}

/// Command-line arguments for the cost-matrix command.
#[derive(Args, Debug)]
pub struct CostMatrixArgs {
    #[arg(value_name = "CONTRACT_ID")]
    pub contract_id: String,

    /// TTL targets in days, separated by commas
    #[arg(long, value_delimiter = ',', default_value = "30,90,180,365", value_parser = clap::value_parser!(u32).range(1..))]
    pub tiers: Vec<u32>,

    #[arg(long, default_value = "testnet")]
    pub network: String,

    #[arg(long, env = "SOROBAN_RPC_URL")]
    pub rpc_url: Option<String>,

    #[arg(long, default_value = "text", value_parser = ["text", "json"])]
    pub format: String,

    /// Persistent storage key symbol (repeatable)
    #[arg(long)]
    pub key: Vec<String>,

    /// Assumed effective rent fee per KiB in stroops
    #[arg(long, default_value_t = 1000)]
    pub rent_rate_per_kib: u64,

    /// Assumed persistent rent divisor
    #[arg(long, default_value_t = 1215, value_parser = clap::value_parser!(u32).range(1..))]
    pub persistent_rent_denominator: u32,

    /// Assumed non-rent fee per batched transaction in stroops
    #[arg(long, default_value_t = 100)]
    pub transaction_fee: u64,

    /// Assumed network base reserve in stroops
    #[arg(long, default_value_t = 5_000_000)]
    pub base_reserve: u64,
}

/// Command-line arguments for the `sentinel bundle` command.
#[derive(Args, Debug, Clone)]
pub struct BundleArgs {
    /// Contract address to include (repeatable; --key values apply to each)
    #[arg(
        long = "contract",
        value_name = "CONTRACT_ID",
        required_unless_present = "manifest"
    )]
    pub contracts: Vec<String>,

    /// JSON manifest containing contracts and their optional persistent symbol keys
    #[arg(long, value_name = "PATH", conflicts_with = "contracts")]
    pub manifest: Option<std::path::PathBuf>,

    /// Persistent storage symbol key to include for every selected contract
    #[arg(long = "key", value_name = "SYMBOL")]
    pub keys: Vec<String>,

    /// Persistent storage key encoded as base64 ScVal XDR
    #[arg(long = "key-xdr", value_name = "BASE64_XDR")]
    pub key_xdr: Vec<String>,

    /// Stellar account that will be the unsigned transaction source
    #[arg(long = "source-account", value_name = "G...")]
    pub source_account: String,

    /// Extend entries to this many ledgers beyond the current ledger
    #[arg(long = "extend-by", value_name = "LEDGERS")]
    pub extend_by: u32,

    /// Target Stellar network
    #[arg(long, default_value = "testnet", value_name = "NETWORK")]
    pub network: String,

    /// Custom Soroban RPC endpoint URL
    #[arg(long, value_name = "URL", env = "SOROBAN_RPC_URL")]
    pub rpc_url: Option<String>,

    /// Inclusion fee per operation in stroops
    #[arg(long = "base-fee", default_value_t = 100, value_name = "STROOPS")]
    pub base_fee: u32,
}

/// Command-line arguments for the `sentinel restore` command.
#[derive(Args, Debug, Clone)]
pub struct RestoreArgs {
    /// Target Soroban smart contract address (C... Strkey format)
    #[arg(value_name = "CONTRACT_ID")]
    pub contract_id: String,

    /// Persistent storage key symbol to restore (can be specified multiple times)
    #[arg(long = "key", value_name = "SYMBOL")]
    pub key: Vec<String>,

    /// Persistent storage key encoded as base64 ScVal XDR (can be specified multiple times)
    #[arg(long = "key-xdr", value_name = "BASE64_XDR")]
    pub key_xdr: Vec<String>,

    /// Persistent storage key encoded as typed expression (can be specified multiple times)
    #[arg(long = "key-expr", value_name = "EXPR")]
    pub key_expr: Vec<String>,

    /// Source Stellar account used for the simulated transaction (defaults to a placeholder)
    #[arg(short = 's', long, value_name = "G...")]
    pub source_account: Option<String>,

    /// Base transaction fee in stroops (resource fee is added after simulation)
    #[arg(long, default_value_t = 100, value_name = "STROOPS")]
    pub base_fee: u32,

    /// Target Stellar network
    #[arg(long, default_value = "testnet", value_name = "NETWORK")]
    pub network: String,

    /// Output format: "text" (human readable) or "json" (machine readable)
    #[arg(long, default_value = "text", value_parser = ["text", "json"], value_name = "FORMAT")]
    pub format: String,

    /// Custom Soroban RPC endpoint URL
    #[arg(long, value_name = "URL", env = "SOROBAN_RPC_URL")]
    pub rpc_url: Option<String>,
}

/// Command-line arguments for a one-shot manifest check.
#[derive(Args, Debug)]
#[command(
    after_help = "Single-contract monitoring exits: 0 OK, 1 WARNING, 2 CRITICAL, 3 UNKNOWN. Fleet mode exits: 0 OK, 1 warning, 2 critical or archived, 10 invalid input, 11 RPC failure."
)]
pub struct CheckArgs {
    /// Single contract to check with Nagios exit codes and performance data
    #[arg(value_name = "CONTRACT_ID")]
    pub contract_id: Option<String>,
    #[command(flatten)]
    pub scan: StorageScanOptions,
    /// Path to the fleet manifest
    #[arg(
        long,
        default_value = "sentinel.toml",
        value_name = "PATH",
        conflicts_with = "contract_id"
    )]
    pub config: std::path::PathBuf,

    /// Report format
    #[arg(long, default_value = "text", value_parser = ["text", "json"])]
    pub format: String,

    /// Disable ANSI color codes in text output (also honored via the NO_COLOR environment variable)
    #[arg(long = "no-color")]
    pub no_color: bool,
}

/// Shared selection for read-only contract entry inspection.
#[derive(Args, Debug, Clone)]
pub struct StorageScanOptions {
    #[arg(long, env = "SOROBAN_RPC_URL")]
    pub rpc_url: Option<String>,
    #[arg(long, default_value = "testnet")]
    pub network: String,
    #[arg(long = "key")]
    pub key: Vec<String>,
    #[arg(long = "key-xdr")]
    pub key_xdr: Vec<String>,
    #[arg(long = "key-expr")]
    pub key_expr: Vec<String>,
    #[arg(long, default_value_t = 120960)]
    pub warning_threshold: u32,
    #[arg(long)]
    pub critical_threshold: Option<u32>,
}

#[derive(Args, Debug, Clone)]
pub struct TopConsumersArgs {
    #[arg(value_name = "CONTRACT_ID")]
    pub contract_id: String,
    #[arg(long, default_value_t = 10)]
    pub limit: usize,
    #[arg(long, default_value = "text", value_parser = ["text", "json"])]
    pub format: String,
    #[command(flatten)]
    pub scan: StorageScanOptions,
}

/// Command-line arguments for the `sentinel audit-keys` command.
#[derive(Args, Debug, Clone)]
pub struct AuditKeysArgs {
    /// Target Soroban smart contract address (C... Strkey format)
    #[arg(value_name = "CONTRACT_ID")]
    pub contract_id: String,

    /// Custom Soroban RPC endpoint URL (overrides the network preset)
    #[arg(long, value_name = "URL", env = "SOROBAN_RPC_URL")]
    pub rpc_url: Option<String>,

    /// Target Stellar network
    #[arg(long, default_value = "testnet", value_name = "NETWORK")]
    pub network: String,

    /// Output format: text or json
    #[arg(long, default_value = "text", value_parser = ["text", "json"])]
    pub format: String,

    /// Persistent storage key symbols to inspect
    #[arg(long = "key", value_name = "SYMBOL")]
    pub key: Vec<String>,

    /// Persistent storage key encoded as base64 ScVal XDR
    #[arg(long = "key-xdr", value_name = "BASE64_XDR")]
    pub key_xdr: Vec<String>,

    /// Persistent storage key typed expression
    #[arg(long = "key-expr", value_name = "EXPR")]
    pub key_expr: Vec<String>,

    /// Temporary storage key symbols to inspect
    #[arg(long = "temp-key", value_name = "SYMBOL")]
    pub temp_key: Vec<String>,

    /// Temporary storage key typed expression
    #[arg(long = "temp-key-expr", value_name = "EXPR")]
    pub temp_key_expr: Vec<String>,

    /// Custom RPC request timeout in seconds
    #[arg(long = "rpc-timeout", value_name = "SECS")]
    pub rpc_timeout: Option<u64>,

    /// Maximum RPC retry attempts
    #[arg(long = "rpc-max-retries", value_name = "N")]
    pub rpc_max_retries: Option<u32>,
}

/// Command-line arguments for the `sentinel bump` command.
#[derive(Args, Debug, Clone)]
pub struct BumpArgs {
    /// Contract address whose selected entries will be extended.
    #[arg(value_name = "CONTRACT_ID")]
    pub contract_id: String,

    /// Persistent storage key symbol (repeatable).
    #[arg(long = "key", value_name = "SYMBOL")]
    pub key: Vec<String>,

    /// Base64 XDR storage key (repeatable).
    #[arg(long = "key-xdr", value_name = "BASE64_XDR")]
    pub key_xdr: Vec<String>,

    /// Ledger TTL extension target.
    #[arg(long, value_name = "LEDGERS")]
    pub extend_to: u32,

    #[arg(long, default_value = "testnet")]
    pub network: String,

    #[arg(long, value_name = "URL", env = "SOROBAN_RPC_URL")]
    pub rpc_url: Option<String>,

    /// Source Stellar account G... address.
    #[arg(long, value_name = "G...")]
    pub source_account: String,

    /// Maximum total fee; required for submission.
    #[arg(long, value_name = "STROOPS")]
    pub max_fee: Option<u64>,

    /// Submit the signed transaction instead of dry-run only.
    #[arg(long)]
    pub submit: bool,

    /// Network passphrase required when a custom RPC URL is used.
    #[arg(long, value_name = "PASSPHRASE")]
    pub network_passphrase: Option<String>,
}

/// Command-line arguments for `sentinel network`.
#[derive(Args, Debug, Clone)]
pub struct NetworkArgs {
    #[arg(long, default_value = "testnet", value_name = "NETWORK")]
    pub network: String,

    #[arg(long, value_name = "URL", env = "SOROBAN_RPC_URL")]
    pub rpc_url: Option<String>,

    #[arg(long, default_value = "text", value_parser = ["text", "json"])]
    pub format: String,
}

/// Command-line arguments for `sentinel install-service`.
#[derive(Args, Debug, Clone)]
pub struct InstallServiceArgs {
    /// Generate a systemd .service unit (mutually exclusive with --launchd)
    #[arg(long, conflicts_with = "launchd")]
    pub systemd: bool,

    /// Generate a macOS launchd .plist (mutually exclusive with --systemd)
    #[arg(long, conflicts_with = "systemd")]
    pub launchd: bool,

    /// Path to the TOML fleet manifest
    #[arg(long, value_name = "PATH")]
    pub manifest: std::path::PathBuf,

    /// User account used by the service
    #[arg(long, value_name = "USER")]
    pub user: Option<String>,

    /// Directory in which to write the generated service file
    #[arg(long, value_name = "DIR")]
    pub output_dir: Option<std::path::PathBuf>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_scan_defaults() {
        let args = Cli::parse_from([
            "sentinel",
            "scan",
            "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM",
        ]);


        assert!(args.otlp_endpoint.is_none());
        match args.command {
            Commands::Scan(scan) => {
                assert_eq!(
                    scan.contract_id,
                    "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM"
                );
                assert_eq!(scan.network, "testnet");
                assert_eq!(scan.format, "text");
                assert!(!scan.no_color);
                assert!(scan.output.is_none());
                assert_eq!(scan.threshold, 120960);
                assert!(scan.rpc_url.is_none());
                assert!(scan.key.is_empty());
                assert!(scan.key_xdr.is_empty());
                assert!(scan.key_expr.is_empty());
                assert!(scan.rpc_timeout.is_none());
                assert!(scan.rpc_max_retries.is_none());
                assert!(scan.ledger_seconds.is_none());
                assert_eq!(scan.sample_window, DEFAULT_SAMPLE_WINDOW_LEDGERS);
                assert!(scan.baseline.is_none());
                assert!(!scan.fail_on_regression);
                assert!(!scan.tui);
                assert!(scan.snapshot.is_none());
            }
            _ => panic!("expected scan command"),
        }
    }

    #[test]
    fn test_parse_scan_custom_flags() {
        let args = Cli::parse_from([
            "sentinel",
            "scan",
            "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM",
            "--rpc-url",
            "https://soroban-testnet.stellar.org",
            "--network",
            "mainnet",
            "--format",
            "ndjson",
            "--no-color",
            "--output",
            "report.csv",
            "--threshold",
            "50000",
            "--key",
            "COUNTER",
            "--key",
            "ADMIN",
            "--key-xdr",
            "AAAAFA==",
            "--key-expr",
            "sym:Balance",
            "--key-expr",
            "vec[sym:Data, u32:1]",
            "--rpc-timeout",
            "30",
            "--rpc-max-retries",
            "5",
            "--ledger-seconds",
            "5.5",
            "--sample-window",
            "250",
            "--baseline",
            "baseline.json",
            "--fail-on-regression",
            "--tui",
            "--otlp-endpoint",
            "http://localhost:4317",
        ]);

        assert_eq!(args.otlp_endpoint.as_deref(), Some("http://localhost:4317"));
        match args.command {
            Commands::Scan(scan) => {
                assert_eq!(scan.network, "mainnet");
                assert_eq!(scan.format, "ndjson");
                assert!(scan.no_color);
                assert_eq!(scan.output.as_deref(), Some("report.csv"));
                assert_eq!(scan.threshold, 50000);
                assert_eq!(scan.key, vec!["COUNTER", "ADMIN"]);
                assert_eq!(scan.key_xdr, vec!["AAAAFA=="]);
                assert_eq!(
                    scan.key_expr,
                    vec!["sym:Balance", "vec[sym:Data, u32:1]"]
                );
                assert_eq!(scan.rpc_timeout, Some(30));
                assert_eq!(scan.rpc_max_retries, Some(5));
                assert_eq!(scan.ledger_seconds, Some(5.5));
                assert_eq!(scan.sample_window, 250);
                assert_eq!(scan.baseline.as_deref(), Some("baseline.json"));
                assert!(scan.fail_on_regression);
                assert!(scan.tui);
                assert!(scan.snapshot.is_none());
            }
            _ => panic!("expected scan command"),
        }
    }

    #[test]
    fn test_parse_bundle_arguments() {
        let args = Cli::parse_from([
            "sentinel",
            "bundle",
            "--contract",
            "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM",
            "--contract",
            "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM",
            "--key",
            "ADMIN",
            "--key",
            "COUNTER",
            "--source-account",
            "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
            "--extend-by",
            "100000",
            "--base-fee",
            "200",
        ]);

        match args.command {
            Commands::Bundle(bundle) => {
                assert_eq!(bundle.contracts.len(), 2);
                assert_eq!(bundle.keys, vec!["ADMIN", "COUNTER"]);
                assert_eq!(bundle.extend_by, 100000);
                assert_eq!(bundle.base_fee, 200);
                assert_eq!(bundle.network, "testnet");
            }
            _ => panic!("expected bundle command"),
    fn test_parse_restore_defaults_and_flags() {
        let contract = "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM";
        let defaults = Cli::parse_from(["sentinel", "restore", contract]);
        match defaults.command {
            Commands::Restore(restore) => {
                assert_eq!(restore.contract_id, contract);
                assert!(restore.key.is_empty());
                assert!(restore.key_xdr.is_empty());
                assert!(restore.key_expr.is_empty());
                assert!(restore.source_account.is_none());
                assert_eq!(restore.base_fee, 100);
                assert_eq!(restore.network, "testnet");
                assert_eq!(restore.format, "text");
                assert!(restore.rpc_url.is_none());
            }
            _ => panic!("expected restore command"),
        }

        let custom = Cli::parse_from([
            "sentinel",
            "restore",
            contract,
            "--key",
            "ADMIN",
            "--key-xdr",
            "AAAAFA==",
            "--key-expr",
            "sym:Balance",
            "--source-account",
            "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
            "--base-fee",
            "200",
            "--network",
            "mainnet",
            "--format",
            "json",
        ]);
        match custom.command {
            Commands::Restore(restore) => {
                assert_eq!(restore.key, vec!["ADMIN"]);
                assert_eq!(restore.key_xdr, vec!["AAAAFA=="]);
                assert_eq!(restore.key_expr, vec!["sym:Balance"]);
                assert_eq!(restore.base_fee, 200);
                assert_eq!(restore.network, "mainnet");
                assert_eq!(restore.format, "json");
            }
            _ => panic!("expected restore command"),
        }
    }
}
