use clap::{Args, Parser, Subcommand, ValueEnum};

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
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Scan a contract and inspect storage entry TTL status and expiry risk
    Scan(ScanArgs),

    /// Continuously monitor one contract, printing state changes between scans
    Watch(WatchArgs),

    /// Bundle contract and persistent-key TTL extensions into simulated transactions
    Bundle(BundleArgs),

    /// Scan contracts once from a TOML manifest
    Check(CheckArgs),
}

/// Output format for `sentinel scan` results.
#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    /// Colorized ASCII table
    Text,
    /// Machine-readable JSON
    Json,
    /// SARIF v2.1.0 for GitHub Code Scanning
    Sarif,
    /// GitHub-flavoured Markdown for PR and issue comments
    Markdown,
    /// Prometheus text exposition format for the node_exporter textfile collector
    Prometheus,
}

/// Contract, network, key and threshold flags shared by `sentinel scan` and `sentinel watch`.
#[derive(Args, Debug, Clone)]
pub struct ScanTargetArgs {
    /// Target Soroban smart contract address (C... Strkey format)
    #[arg(value_name = "CONTRACT_ID")]
    pub contract_id: String,

    /// Custom Soroban RPC endpoint URL (overrides default network endpoint)
    #[arg(long, value_name = "URL", env = "SOROBAN_RPC_URL")]
    pub rpc_url: Option<String>,

    /// Target Stellar network: "testnet", "mainnet", "futurenet", or "local"
    #[arg(long, default_value = "testnet", value_name = "NETWORK")]
    pub network: String,

    /// Persistent storage key symbol to inspect (can be specified multiple times)
    #[arg(long = "key", value_name = "SYMBOL")]
    pub key: Vec<String>,

    /// Persistent storage key encoded as raw base64 ScVal XDR (can be specified multiple times)
    #[arg(long = "key-xdr", value_name = "BASE64_XDR")]
    pub key_xdr: Vec<String>,

    /// Persistent storage key encoded as typed expression (can be specified multiple times, e.g. "sym:Balance", "vec[sym:Balance, addr:G...]")
    #[arg(long = "key-expr", value_name = "EXPR")]
    pub key_expr: Vec<String>,

    /// Temporary storage key symbol to inspect (can be specified multiple times). Expired temporary entries are deleted permanently.
    #[arg(long = "temp-key", value_name = "SYMBOL")]
    pub temp_key: Vec<String>,

    /// Temporary storage key encoded as raw base64 ScVal XDR (can be specified multiple times)
    #[arg(long = "temp-key-xdr", value_name = "BASE64_XDR")]
    pub temp_key_xdr: Vec<String>,

    /// Warning threshold in ledgers below which entries trigger WARN status (defaults to 120,960 ledgers ≈ 7 days)
    #[arg(long, default_value_t = 120960, value_name = "LEDGERS")]
    pub threshold: u32,

    /// Critical threshold in ledgers below which entries trigger CRITICAL status (defaults to 34,560 ledgers ≈ 2 days, or threshold / 3 if threshold <= 34,560)
    #[arg(long, value_name = "LEDGERS")]
    pub critical_threshold: Option<u32>,

    /// Instance-storage utilisation (% of the network max contract-data entry size) that raises WARN
    #[arg(
        long = "instance-size-warn-pct",
        default_value_t = sentinel_core::domain::DEFAULT_INSTANCE_SIZE_WARN_PCT,
        value_name = "PCT"
    )]
    pub instance_size_warn_pct: u8,

    /// Instance-storage utilisation (% of the network max contract-data entry size) that raises CRITICAL
    #[arg(
        long = "instance-size-critical-pct",
        default_value_t = sentinel_core::domain::DEFAULT_INSTANCE_SIZE_CRITICAL_PCT,
        value_name = "PCT"
    )]
    pub instance_size_critical_pct: u8,

    /// Custom RPC request timeout in seconds (1–120, defaults to 10)
    #[arg(long = "rpc-timeout", value_name = "SECS")]
    pub rpc_timeout: Option<u64>,

    /// Maximum RPC retry attempts for transient errors (0–10, defaults to 3)
    #[arg(long = "rpc-max-retries", value_name = "N")]
    pub rpc_max_retries: Option<u32>,

    /// Override ledger close velocity in seconds per ledger (1.0–60.0)
    #[arg(long = "ledger-seconds", value_name = "SECS")]
    pub ledger_seconds: Option<f64>,
}

/// Command-line arguments for the `sentinel scan` command.
#[derive(Args, Debug, Clone)]
pub struct ScanArgs {
    #[command(flatten)]
    pub target: ScanTargetArgs,

    /// Output format for scan results. Unknown values are rejected before any RPC call.
    #[arg(
        long,
        value_enum,
        default_value_t = OutputFormat::Text,
        ignore_case = true,
        value_name = "FORMAT"
    )]
    pub format: OutputFormat,

    /// Path to a previous JSON scan report baseline file for comparison
    #[arg(long = "baseline", value_name = "PATH")]
    pub baseline: Option<String>,

    /// Return exit code 2 if any entry regressed in risk status or was removed compared to baseline
    #[arg(long = "fail-on-regression")]
    pub fail_on_regression: bool,
}

/// Output format for `sentinel watch`.
#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchFormat {
    /// Human-readable report, then one line per state transition or heartbeat
    Text,
    /// One JSON object per line (NDJSON) for log shippers
    Json,
}

/// Command-line arguments for the `sentinel watch` command.
#[derive(Args, Debug, Clone)]
#[command(
    after_help = "The first scan prints a full report; later scans print only state transitions (status changes, entries appearing or disappearing, TTL bumps) or a heartbeat line. Ctrl-C or SIGTERM stops the watch and exits with the last scan's risk code (0 OK, 1 warning, 2 critical or archived). Invalid input exits 10; reaching --max-consecutive-failures exits 11."
)]
pub struct WatchArgs {
    #[command(flatten)]
    pub target: ScanTargetArgs,

    /// Seconds between scans (10–86,400)
    #[arg(long, default_value_t = 60, value_name = "SECS")]
    pub interval: u64,

    /// Stop after this many scans (mainly for tests and scripted runs)
    #[arg(long = "max-iterations", value_name = "N")]
    pub max_iterations: Option<u64>,

    /// Exit with code 11 after this many consecutive failed scans (1–100)
    #[arg(
        long = "max-consecutive-failures",
        default_value_t = 5,
        value_name = "N"
    )]
    pub max_consecutive_failures: u32,

    /// Output format: text, or NDJSON (one JSON object per line)
    #[arg(
        long,
        value_enum,
        default_value_t = WatchFormat::Text,
        ignore_case = true,
        value_name = "FORMAT"
    )]
    pub format: WatchFormat,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_watch_args() {
        let args = Cli::parse_from([
            "sentinel",
            "watch",
            "CABC",
            "--key",
            "COUNTER",
            "--temp-key",
            "NONCE",
            "--interval",
            "30",
            "--max-iterations",
            "3",
            "--max-consecutive-failures",
            "2",
            "--format",
            "json",
        ]);
        let Commands::Watch(watch) = args.command else {
            panic!("expected watch command");
        };
        assert_eq!(watch.target.contract_id, "CABC");
        assert_eq!(watch.target.key, vec!["COUNTER"]);
        assert_eq!(watch.target.temp_key, vec!["NONCE"]);
        assert_eq!(watch.interval, 30);
        assert_eq!(watch.max_iterations, Some(3));
        assert_eq!(watch.max_consecutive_failures, 2);
        assert_eq!(watch.format, WatchFormat::Json);

        let defaults = Cli::parse_from(["sentinel", "watch", "CABC"]);
        let Commands::Watch(watch) = defaults.command else {
            panic!("expected watch command");
        };
        assert_eq!(watch.interval, 60);
        assert_eq!(watch.max_iterations, None);
        assert_eq!(watch.max_consecutive_failures, 5);
        assert_eq!(watch.format, WatchFormat::Text);
        assert!(
            Cli::try_parse_from(["sentinel", "watch", "CABC", "--format", "markdown"]).is_err()
        );
    }

    #[test]
    fn test_parse_scan_format_values() {
        for (raw, expected) in [
            ("text", OutputFormat::Text),
            ("json", OutputFormat::Json),
            ("sarif", OutputFormat::Sarif),
            ("markdown", OutputFormat::Markdown),
            ("prometheus", OutputFormat::Prometheus),
            ("JSON", OutputFormat::Json),
        ] {
            let args = Cli::try_parse_from(["sentinel", "scan", "CABC", "--format", raw]).unwrap();
            let Commands::Scan(scan) = args.command else {
                panic!("expected scan command");
            };
            assert_eq!(scan.format, expected, "{raw}");
        }
        assert!(Cli::try_parse_from(["sentinel", "scan", "CABC", "--format", "xml"]).is_err());
    }

    #[test]
    fn test_parse_scan_defaults() {
        let args = Cli::parse_from([
            "sentinel",
            "scan",
            "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM",
        ]);
        match args.command {
            Commands::Scan(scan) => {
                assert_eq!(
                    scan.target.contract_id,
                    "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM"
                );
                assert_eq!(scan.target.network, "testnet");
                assert_eq!(scan.format, OutputFormat::Text);
                assert_eq!(scan.target.threshold, 120960);
                assert!(scan.target.rpc_url.is_none());
                assert!(scan.target.key.is_empty());
                assert!(scan.target.key_xdr.is_empty());
                assert!(scan.target.key_expr.is_empty());
                assert!(scan.target.temp_key.is_empty());
                assert!(scan.target.temp_key_xdr.is_empty());
                assert_eq!(scan.target.instance_size_warn_pct, 75);
                assert_eq!(scan.target.instance_size_critical_pct, 90);
                assert!(scan.target.rpc_timeout.is_none());
                assert!(scan.target.rpc_max_retries.is_none());
                assert!(scan.target.ledger_seconds.is_none());
                assert!(scan.baseline.is_none());
                assert!(!scan.fail_on_regression);
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
            "--temp-key",
            "NONCE",
            "--temp-key",
            "SESSION",
            "--temp-key-xdr",
            "AAAAAw==",
            "--rpc-timeout",
            "30",
            "--rpc-max-retries",
            "5",
            "--ledger-seconds",
            "5.5",
            "--baseline",
            "baseline.json",
            "--fail-on-regression",
        ]);

        match args.command {
            Commands::Scan(scan) => {
                assert_eq!(
                    scan.target.contract_id,
                    "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM"
                );
                assert_eq!(
                    scan.target.rpc_url.as_deref(),
                    Some("https://soroban-testnet.stellar.org")
                );
                assert_eq!(scan.target.network, "mainnet");
                assert_eq!(scan.format, OutputFormat::Json);
                assert_eq!(scan.target.threshold, 50000);
                assert_eq!(scan.target.key, vec!["COUNTER", "ADMIN"]);
                assert_eq!(scan.target.key_xdr, vec!["AAAAFA=="]);
                assert_eq!(
                    scan.target.key_expr,
                    vec!["sym:Balance", "vec[sym:Data, u32:1]"]
                );
                assert_eq!(scan.target.temp_key, vec!["NONCE", "SESSION"]);
                assert_eq!(scan.target.temp_key_xdr, vec!["AAAAAw=="]);
                assert_eq!(scan.target.rpc_timeout, Some(30));
                assert_eq!(scan.target.rpc_max_retries, Some(5));
                assert_eq!(scan.target.ledger_seconds, Some(5.5));
                assert_eq!(scan.baseline.as_deref(), Some("baseline.json"));
                assert!(scan.fail_on_regression);
            }
            _ => panic!("expected scan command"),
        }
    }
}
