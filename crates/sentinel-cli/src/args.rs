use clap::{Args, Parser, Subcommand};

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
    Scan(ScanArgs),
    /// Compare the cost of extending contract storage TTL across duration tiers
    CostMatrix(CostMatrixArgs),

    /// Bundle contract and persistent-key TTL extensions into simulated transactions
    Bundle(BundleArgs),

    /// Detect archived persistent keys and assemble CAP-0046 restoration transactions
    Restore(RestoreArgs),

    /// Scan contracts once from a TOML manifest
    Check(CheckArgs),

    /// Generate a systemd or launchd service definition for autonomous watching
    InstallService(InstallServiceArgs),
}

/// Command-line arguments for the `sentinel scan` command.
#[derive(Args, Debug, Clone)]
pub struct ScanArgs {
    /// Target Soroban smart contract address (C... Strkey format)
    #[arg(value_name = "CONTRACT_ID")]
    pub contract_id: String,

    /// Custom Soroban RPC endpoint URL (overrides default network endpoint)
    #[arg(long, value_name = "URL", env = "SOROBAN_RPC_URL")]
    pub rpc_url: Option<String>,

    /// Target Stellar network: "testnet", "mainnet", "futurenet", or "local"
    #[arg(long, default_value = "testnet", value_name = "NETWORK")]
    pub network: String,

    /// Output format for scan results: "text" (colorized ASCII table), "json" (machine-readable), "csv" (flat tabular export), or "sarif" (GitHub Code Scanning)
    #[arg(long, default_value = "text", value_name = "FORMAT")]
    pub format: String,

    /// Write report output to a file instead of stdout (existing files are overwritten)
    #[arg(long, value_name = "PATH")]
    pub output: Option<String>,

    /// Persistent storage key symbol to inspect (can be specified multiple times)
    #[arg(long = "key", value_name = "SYMBOL")]
    pub key: Vec<String>,

    /// Persistent storage key encoded as raw base64 ScVal XDR (can be specified multiple times)
    #[arg(long = "key-xdr", value_name = "BASE64_XDR")]
    pub key_xdr: Vec<String>,

    /// Persistent storage key encoded as typed expression (can be specified multiple times, e.g. "sym:Balance", "vec[sym:Balance, addr:G...]")
    #[arg(long = "key-expr", value_name = "EXPR")]
    pub key_expr: Vec<String>,

    /// Warning threshold in ledgers below which entries trigger WARN status (defaults to 120,960 ledgers ≈ 7 days)
    #[arg(long, default_value_t = 120960, value_name = "LEDGERS")]
    pub threshold: u32,

    /// Critical threshold in ledgers below which entries trigger CRITICAL status (defaults to 34,560 ledgers ≈ 2 days, or threshold / 3 if threshold <= 34,560)
    #[arg(long, value_name = "LEDGERS")]
    pub critical_threshold: Option<u32>,

    /// Custom RPC request timeout in seconds (1–120, defaults to 10)
    #[arg(long = "rpc-timeout", value_name = "SECS")]
    pub rpc_timeout: Option<u64>,

    /// Maximum RPC retry attempts for transient errors (0–10, defaults to 3)
    #[arg(long = "rpc-max-retries", value_name = "N")]
    pub rpc_max_retries: Option<u32>,

    /// Override ledger close velocity in seconds per ledger (1.0–60.0)
    #[arg(long = "ledger-seconds", value_name = "SECS")]
    pub ledger_seconds: Option<f64>,

    /// Path to a previous JSON scan report baseline file for comparison
    #[arg(long = "baseline", value_name = "PATH")]
    pub baseline: Option<String>,

    /// Return exit code 2 if any entry regressed in risk status or was removed compared to baseline
    #[arg(long = "fail-on-regression")]
    pub fail_on_regression: bool,

    /// Launch the interactive TUI dashboard (requires the `tui` Cargo feature and a TTY)
    #[arg(long)]
    pub tui: bool,
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
    /// Contract addresses to include (can be specified multiple times)
    #[arg(long = "contract", value_name = "CONTRACT_ID")]
    pub contracts: Vec<String>,

    /// Persistent storage key symbol applied to each contract
    #[arg(long = "key", value_name = "SYMBOL")]
    pub keys: Vec<String>,

    /// Persistent storage key encoded as base64 ScVal XDR
    #[arg(long = "key-xdr", value_name = "BASE64_XDR")]
    pub key_xdr: Vec<String>,

    /// JSON bundle manifest path
    #[arg(long, value_name = "PATH")]
    pub manifest: Option<std::path::PathBuf>,

    /// Source Stellar account that will sign the transaction
    #[arg(long, value_name = "G...")]
    pub source_account: String,

    /// Ledger TTL extension length
    #[arg(long, value_name = "LEDGERS")]
    pub extend_by: u32,

    /// Base transaction fee in stroops
    #[arg(long, default_value_t = 100, value_name = "STROOPS")]
    pub base_fee: u32,

    /// Target Stellar network
    #[arg(long, default_value = "testnet", value_name = "NETWORK")]
    pub network: String,

    /// Custom Soroban RPC endpoint URL
    #[arg(long, value_name = "URL", env = "SOROBAN_RPC_URL")]
    pub rpc_url: Option<String>,
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
    after_help = "Exit codes: 0 OK, 1 warning, 2 critical or archived, 10 invalid input, 11 RPC failure. Any RPC failure takes precedence over risk results."
)]
pub struct CheckArgs {
    /// Path to the fleet manifest
    #[arg(long, default_value = "sentinel.toml", value_name = "PATH")]
    pub config: std::path::PathBuf,

    /// Report format
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
                assert!(scan.output.is_none());
                assert_eq!(scan.threshold, 120960);
                assert!(scan.rpc_url.is_none());
                assert!(scan.key.is_empty());
                assert!(scan.key_xdr.is_empty());
                assert!(scan.key_expr.is_empty());
                assert!(scan.rpc_timeout.is_none());
                assert!(scan.rpc_max_retries.is_none());
                assert!(scan.ledger_seconds.is_none());
                assert!(scan.baseline.is_none());
                assert!(!scan.fail_on_regression);
                assert!(!scan.tui);
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
            "json",
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
                assert_eq!(
                    scan.contract_id,
                    "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM"
                );
                assert_eq!(
                    scan.rpc_url.as_deref(),
                    Some("https://soroban-testnet.stellar.org")
                );
                assert_eq!(scan.network, "mainnet");
                assert_eq!(scan.format, "json");
                assert_eq!(scan.output.as_deref(), Some("report.csv"));
                assert_eq!(scan.threshold, 50000);
                assert_eq!(scan.key, vec!["COUNTER", "ADMIN"]);
                assert_eq!(scan.key_xdr, vec!["AAAAFA=="]);
                assert_eq!(scan.key_expr, vec!["sym:Balance", "vec[sym:Data, u32:1]"]);
                assert_eq!(scan.rpc_timeout, Some(30));
                assert_eq!(scan.rpc_max_retries, Some(5));
                assert_eq!(scan.ledger_seconds, Some(5.5));
                assert_eq!(scan.baseline.as_deref(), Some("baseline.json"));
                assert!(scan.fail_on_regression);
                assert!(scan.tui);
            }
            _ => panic!("expected scan command"),
        }
    }

    #[test]
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
