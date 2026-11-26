use clap::{ArgAction, Args, Parser, Subcommand, ValueEnum};
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

    /// Enable verbose diagnostics on stderr. Use -v for summary, -vv for detailed (retries/backoff).
    #[arg(short = 'v', global = true, action = ArgAction::Count)]
    pub verbose: u8,

    /// Suppress non-error stderr output. Only show errors.
    #[arg(short = 'q', global = true)]
    pub quiet: bool,

    /// Output format for diagnostics: "text" (plain text) or "json" (one JSON object per line).
    #[arg(long = "log-format", global = true, value_parser = ["text", "json"], default_value = "text")]
    pub log_format: String,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Scan a contract and inspect storage entry TTL status and expiry risk
    Scan(Box<ScanArgs>),
    /// Export OpenAPI 3.1 webhook contracts offline
    ExportOpenapi(ExportOpenapiArgs),
    /// Score contract health from a fleet manifest
    HealthMatrix(HealthMatrixArgs),
    /// Benchmark Soroban RPC endpoint performance (latency, throughput, percentiles)
    Benchmark(BenchmarkArgs),
    /// Rank resolved contract entries by serialized byte size
    TopConsumers(TopConsumersArgs),
    /// Check Soroban RPC connectivity, network identity, and protocol support
    Doctor(DoctorArgs),

    /// Measure the connection and response latency of a Soroban RPC endpoint
    TraceRoute(TraceRouteArgs),
    /// Monitor one contract and notify configured incident providers
    Watch(Box<WatchArgs>),

    /// Lint a manifest file and export JSON Schema for IDE autocompletion
    LintManifest(LintManifestArgs),

    /// Export JSON Schema for IDE autocompletion (VS Code / IntelliJ)
    Schema(SchemaArgs),

    /// Compare the cost of extending contract storage TTL across duration tiers
    CostMatrix(CostMatrixArgs),

    /// Bundle contract and persistent-key TTL extensions into simulated transactions
    Bundle(BundleArgs),

    /// Estimate rent reserve requirements for a contract storage horizon
    Budget(BudgetArgs),

    /// Export Grafana dashboard and Prometheus alerting definitions
    ExportGrafana(ExportGrafanaArgs),

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

    /// Generate shell completion scripts for the `sentinel` CLI
    Completions(CompletionsArgs),
    /// Generate a sentinel.toml fleet manifest
    Init(InitArgs),

    /// Generate Docker deployment files
    GenerateDocker(GenerateDockerArgs),

    /// Encode a Soroban storage key expression as XDR
    KeyEncode(KeyEncodeArgs),

    /// Decode a base64 Soroban storage key XDR value
    KeyDecode(KeyDecodeArgs),

    /// Compare two saved JSON TTL reports without using RPC
    DiffReports(DiffReportsArgs),

    /// Export the monitored keys in a saved JSON TTL report as sentinel.toml or JSON
    ExportKeys(ExportKeysArgs),

    /// Decode a base64 LedgerEntry, LedgerEntryData, or LedgerKey XDR value
    Explain(ExplainArgs),

    /// Compare a contract's storage health across two Stellar networks
    DiffNetworks(DiffNetworksArgs),

    /// Compare storage keys between two saved scan reports
    KeyDiff(KeyDiffArgs),

    /// Estimate the fee for extending a contract footprint TTL
    Forecast(ForecastArgs),

    /// Display network state-archival and contract entry limits
    Network(NetworkArgs),
    /// Remove sensitive storage keys and payloads from a saved scan report
    Redact(RedactArgs),
    /// Validate contract and key arguments, then check RPC health
    DryRun(DryRunArgs),
    /// Estimate a contract TTL extension's resource limits
    EstimateGas(EstimateGasArgs),

    /// Verify contract storage compliance against Stellar protocol ceilings
    AuditLimits(AuditLimitsArgs),

    /// Scaffold an automated GitHub Actions CI workflow for TTL drift scanning
    GenerateGithubWorkflow(GenerateWorkflowArgs),
    /// Analyze historical scan snapshots to project TTL decay and threshold crossings
    Trend(TrendArgs),
}

/// Command-line arguments for `sentinel generate-docker`.
#[derive(Args, Debug, Clone)]
pub struct GenerateDockerArgs {
    /// Directory where Docker deployment files are written
    #[arg(long = "out-dir", default_value = ".", value_name = "PATH")]
    pub out_dir: std::path::PathBuf,
}

#[derive(Args, Debug)]
pub struct TraceRouteArgs {
    #[arg(long, value_name = "URL")]
    pub rpc_url: String,
    /// Emit timing measurements as JSON
    #[arg(long)]
    pub json: bool,
    /// Disable ANSI color codes in terminal output
    #[arg(long)]
    pub no_color: bool,
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

#[derive(Args, Clone)]
pub struct RedactArgs {
    #[arg(long, value_name = "SCAN_REPORT.json")]
    pub input: std::path::PathBuf,
    #[arg(long, value_name = "REDACTED.json")]
    pub output: Option<std::path::PathBuf>,
    /// Secret key for stable HMAC pseudonyms across reports
    #[arg(long, hide_env_values = true, env = "SENTINEL_REDACT_SALT")]
    pub salt: Option<String>,
}

impl std::fmt::Debug for RedactArgs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RedactArgs")
            .field("input", &self.input)
            .field("output", &self.output)
            .field("salt", &self.salt.as_ref().map(|_| "[REDACTED]"))
            .finish()
    }
}

#[derive(Args, Debug, Clone)]
pub struct DryRunArgs {
    #[arg(value_name = "CONTRACT_ID")]
    pub contract_id: String,
    #[arg(long, value_name = "URL", env = "SOROBAN_RPC_URL")]
    pub rpc_url: Option<String>,
    #[arg(long = "key-expr", value_name = "EXPR")]
    pub key_expr: Vec<String>,
}

#[derive(Args, Debug, Clone)]
pub struct EstimateGasArgs {
    #[arg(value_name = "CONTRACT_ID")]
    pub contract_id: String,
    #[arg(long = "extend-by", value_name = "LEDGERS")]
    pub extend_by: u32,
    #[arg(long, default_value = "testnet", value_name = "NETWORK")]
    pub network: String,
    #[arg(long, value_name = "URL", env = "SOROBAN_RPC_URL")]
    pub rpc_url: Option<String>,
    #[arg(long = "key", value_name = "SYMBOL")]
    pub key: Vec<String>,
    #[arg(long = "key-xdr", value_name = "BASE64_XDR")]
    pub key_xdr: Vec<String>,
    #[arg(long = "key-expr", value_name = "EXPR")]
    pub key_expr: Vec<String>,
    #[arg(long, default_value = "text", value_parser = ["text", "json"])]
    pub format: String,
}

// Avoid Debug implementations that print provider credentials.
#[derive(Args)]
pub struct WatchArgs {
    #[command(flatten)]
    pub scan: ScanArgs,
    /// Seconds between scans
    #[arg(long, default_value_t = 60, value_parser = clap::value_parser!(u64).range(1..))]
    pub interval: u64,
    /// Matrix homeserver URL for room alerts
    #[arg(long = "alert-matrix-homeserver", value_name = "URL")]
    pub alert_matrix_homeserver: Option<String>,
    /// Matrix room ID for room alerts
    #[arg(long = "alert-matrix-room", value_name = "ROOM_ID")]
    pub alert_matrix_room: Option<String>,
    /// Matrix access token for room alerts
    #[arg(
        long = "alert-matrix-token",
        env = "SENTINEL_ALERT_MATRIX_TOKEN",
        value_name = "TOKEN",
        hide_env_values = true
    )]
    pub alert_matrix_token: Option<String>,
    #[arg(long, env = "PAGERDUTY_ROUTING_KEY", hide_env_values = true)]
    pub pagerduty_routing_key: Option<String>,
    #[arg(long, env = "OPSGENIE_API_KEY", hide_env_values = true)]
    pub opsgenie_api_key: Option<String>,
    /// Mattermost incoming webhook URL for formatted alert cards
    #[arg(
        long = "alert-mattermost",
        env = "SENTINEL_ALERT_MATTERMOST",
        value_name = "URL",
        hide_env_values = true
    )]
    pub alert_mattermost: Option<String>,
    /// Optional Mattermost channel override for incoming webhook alerts
    #[arg(long = "alert-mattermost-channel", value_name = "CHANNEL")]
    pub alert_mattermost_channel: Option<String>,
    /// Optional Mattermost username override for incoming webhook alerts
    #[arg(long = "alert-mattermost-username", value_name = "USERNAME")]
    pub alert_mattermost_username: Option<String>,
    /// Telegram Bot API token for outbound risk alerts
    #[arg(long, env = "SENTINEL_TELEGRAM_BOT_TOKEN", hide_env_values = true)]
    pub telegram_bot_token: Option<String>,
    /// Telegram target chat or channel ID for outbound risk alerts
    #[arg(
        long,
        env = "SENTINEL_TELEGRAM_CHAT_ID",
        hide_env_values = true,
        allow_hyphen_values = true
    )]
    pub telegram_chat_id: Option<String>,
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
    /// Google Chat incoming webhook URL for risk alerts
    #[arg(
        long = "alert-google-chat",
        value_name = "WEBHOOK_URL",
        hide_env_values = true
    )]
    pub alert_google_chat: Option<String>,
    /// Microsoft Teams incoming webhook URL for Adaptive Card alerts
    #[arg(
        long = "alert-teams",
        env = "TEAMS_WEBHOOK_URL",
        hide_env_values = true,
        value_name = "WEBHOOK_URL"
    )]
    pub alert_teams: Option<String>,
    /// SMTP server address for email notifications
    #[arg(
        long = "smtp-server",
        value_name = "HOST",
        env = "SENTINEL_SMTP_SERVER"
    )]
    pub smtp_server: Option<String>,
    /// SMTP port for email notifications (default: 587)
    #[arg(long = "smtp-port", default_value_t = 587, value_name = "PORT")]
    pub smtp_port: u16,
    /// SMTP username for email notifications
    #[arg(
        long = "smtp-username",
        value_name = "USER",
        env = "SENTINEL_SMTP_USERNAME"
    )]
    pub smtp_username: Option<String>,
    /// SMTP password for email notifications
    #[arg(
        long = "smtp-password",
        value_name = "PASS",
        env = "SENTINEL_SMTP_PASSWORD"
    )]
    pub smtp_password: Option<String>,
    /// Recipient email addresses (specify multiple times for multiple recipients)
    #[arg(
        long = "smtp-recipient",
        value_name = "EMAIL",
        env = "SENTINEL_SMTP_RECIPIENT"
    )]
    pub smtp_recipients: Vec<String>,
    /// Use STARTTLS for SMTP (connect without TLS then upgrade)
    #[arg(long = "smtp-starttls")]
    pub smtp_starttls: bool,
    /// Webhook secret for HMAC-SHA256 signature computation
    #[arg(
        long = "webhook-secret",
        value_name = "SECRET",
        env = "SENTINEL_WEBHOOK_SECRET"
    )]
    pub webhook_secret: Option<String>,
    /// Webhook token for bearer token authorization
    #[arg(
        long = "webhook-token",
        value_name = "TOKEN",
        env = "SENTINEL_WEBHOOK_TOKEN"
    )]
    pub webhook_token: Option<String>,
    /// Webhook URL for alert notifications
    #[arg(long = "webhook-url", value_name = "URL", env = "SENTINEL_WEBHOOK_URL")]
    pub webhook_url: Option<String>,
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

/// Command-line arguments for `sentinel export-keys`.
#[derive(Args, Debug, Clone)]
pub struct ExportKeysArgs {
    /// Saved JSON TTL report (from `sentinel scan --format json`)
    #[arg(long, value_name = "REPORT_JSON")]
    pub input: std::path::PathBuf,

    /// Output format
    #[arg(long, default_value = "toml", value_parser = ["toml", "json"])]
    pub format: String,
}

/// Command-line arguments for `sentinel explain`.
#[derive(Args, Debug, Clone)]
pub struct ExplainArgs {
    /// Base64 XDR for a LedgerEntry, LedgerEntryData, or LedgerKey
    #[arg(
        value_name = "BASE64_XDR",
        required_unless_present = "stdin",
        conflicts_with = "stdin"
    )]
    pub xdr: Option<String>,

    /// Read the base64 XDR from standard input
    #[arg(long)]
    pub stdin: bool,

    /// Output format
    #[arg(long, default_value = "text", value_parser = ["text", "json"])]
    pub format: String,
}

/// Arguments for `sentinel key-diff`.
#[derive(Args, Debug, Clone)]
pub struct KeyDiffArgs {
    /// Earlier scan report
    #[arg(long, value_name = "SCAN_A.json")]
    pub before: std::path::PathBuf,
    /// Later scan report
    #[arg(long, value_name = "SCAN_B.json")]
    pub after: std::path::PathBuf,
    /// Output format
    #[arg(long, default_value = "text", value_parser = ["text", "json"])]
    pub format: String,
    /// Disable ANSI color codes in text output
    #[arg(long = "no-color")]
    pub no_color: bool,
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
/// Contract, network, key and threshold flags for `sentinel scan`.
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

    /// Output format for scan results: "text" (colorized ASCII table), "json" (machine-readable), "ndjson" (line-delimited JSON), "csv" (flat tabular export), "parquet" (binary columnar export, requires the `parquet` Cargo feature), "sarif" (GitHub Code Scanning), "markdown" (CI summary), "html" (self-contained HTML5 report), "prometheus", "pushgateway", or "sonarqube" (SonarQube Generic Issues)
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

    /// Temporary storage key symbol to inspect (can be specified multiple times). Expired temporary entries are deleted permanently.
    #[arg(long = "temp-key", value_name = "SYMBOL")]
    pub temp_key: Vec<String>,

    /// Temporary storage key encoded as raw base64 ScVal XDR (can be specified multiple times)
    #[arg(long = "temp-key-xdr", value_name = "BASE64_XDR")]
    pub temp_key_xdr: Vec<String>,

    /// Warning threshold in ledgers below which entries trigger WARN status (defaults to 120,960 ledgers ≈ 7 days)
    #[arg(long, default_value_t = 120960, value_name = "LEDGERS")]
    pub threshold: u32,

    /// Critical threshold in ledgers
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

    /// Return a nonzero exit code for entries at or above this risk level
    #[arg(long = "fail-on-risk", value_parser = ["critical"])]
    pub fail_on_risk: Option<String>,

    /// Custom RPC request timeout in seconds (1 to 120, defaults to 10)
    #[arg(long = "rpc-timeout", value_name = "SECS")]
    pub rpc_timeout: Option<u64>,

    /// Maximum RPC retry attempts for transient errors (0 to 10, defaults to 3)
    #[arg(long = "rpc-max-retries", value_name = "N")]
    pub rpc_max_retries: Option<u32>,

    /// Override ledger close velocity in seconds per ledger (1.0 to 60.0)
    #[arg(long = "ledger-seconds", value_name = "SECS")]
    pub ledger_seconds: Option<f64>,

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

/// Command-line arguments for the `sentinel lint-manifest` command.
#[derive(Args, Debug, Clone)]
pub struct LintManifestArgs {
    /// Path to the manifest JSON file to validate and generate schema from
    #[arg(long = "manifest", value_name = "PATH")]
    pub manifest: String,
}

/// Command-line arguments for the `sentinel schema` command.
#[derive(Args, Debug, Clone)]
pub struct SchemaArgs {
    /// Path to the manifest JSON file to generate JSON Schema from
    #[arg(long = "manifest", value_name = "PATH")]
    pub manifest: String,
}

/// Command-line arguments for the `sentinel benchmark-rpc` command.
#[derive(Args, Debug, Clone)]
pub struct BenchmarkArgs {
    /// Required RPC endpoint URL (e.g. https://soroban-testnet.stellar.org)
    #[arg(value_name = "URL")]
    pub rpc_url: String,

    /// Number of concurrent requests to issue during benchmark (default: 1)
    #[arg(long = "concurrency", default_value = "1", value_name = "N")]
    pub concurrency: usize,

    /// Benchmark duration in seconds (default: 30s)
    #[arg(long = "duration-secs", default_value = "30", value_name = "SECS")]
    pub duration_secs: u64,

    /// Output format for benchmark results: "text" (human-readable table) or "json" (machine-readable)
    #[arg(long = "format", default_value = "text", value_name = "FORMAT")]
    pub format: String,
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

/// Command-line arguments for the `sentinel budget` command.
#[derive(Args, Debug, Clone)]
pub struct BudgetArgs {
    /// Target Soroban smart contract address
    #[arg(value_name = "CONTRACT_ID")]
    pub contract_id: String,

    /// Target storage horizon in days
    #[arg(long, default_value_t = 30, value_name = "DAYS")]
    pub horizon_days: u32,

    /// Network rent rate in stroops per byte per ledger
    #[arg(long, value_name = "STROOPS_PER_BYTE_LEDGER")]
    pub fee_rate_stroops_per_byte_ledger: u64,

    /// Custom Soroban RPC endpoint URL
    #[arg(long, value_name = "URL", env = "SOROBAN_RPC_URL")]
    pub rpc_url: Option<String>,

    /// Target Stellar network
    #[arg(long, default_value = "testnet", value_name = "NETWORK")]
    pub network: String,

    /// Output format: text or json
    #[arg(long, default_value = "text", value_parser = ["text", "json"])]
    pub format: String,

    /// Persistent storage key symbol to include (repeatable)
    #[arg(long = "key", value_name = "SYMBOL")]
    pub persistent_keys: Vec<String>,

    /// Persistent storage key encoded as base64 ScVal XDR (repeatable)
    #[arg(long = "key-xdr", value_name = "BASE64_XDR")]
    pub persistent_key_xdr: Vec<String>,

    /// Persistent storage key encoded as a typed expression (repeatable)
    #[arg(long = "key-expr", value_name = "EXPR")]
    pub persistent_key_exprs: Vec<String>,

    /// Temporary storage key symbol to include (repeatable)
    #[arg(long = "temporary-key", value_name = "SYMBOL")]
    pub temporary_keys: Vec<String>,

    /// Temporary storage key encoded as base64 ScVal XDR (repeatable)
    #[arg(long = "temporary-key-xdr", value_name = "BASE64_XDR")]
    pub temporary_key_xdr: Vec<String>,

    /// Temporary storage key encoded as a typed expression (repeatable)
    #[arg(long = "temporary-key-expr", value_name = "EXPR")]
    pub temporary_key_exprs: Vec<String>,
}

/// Command-line arguments for the `sentinel export-grafana` command.
#[derive(Args, Debug, Clone)]
pub struct ExportGrafanaArgs {
    /// Directory where dashboard and alert rule files are written
    #[arg(long, default_value = ".", value_name = "DIR")]
    pub output_dir: std::path::PathBuf,
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

    /// Maximum simulated transaction fee allowed when submitting.
    #[arg(long, value_name = "STROOPS")]
    pub max_fee: Option<u64>,

    /// Submit the signed transaction instead of only printing it.
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
/// Shell type for completion script generation.
#[derive(clap::Parser, Debug, Clone, Copy)]
pub enum Shell {
    /// Bash completion script
    Bash,
    /// Zsh completion script
    Zsh,
    /// Fish completion script
    Fish,
    /// PowerShell completion script
    PowerShell,
    /// Elvish completion script
    Elvish,
}

impl std::fmt::Display for Shell {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Shell::Bash => write!(f, "bash"),
            Shell::Zsh => write!(f, "zsh"),
            Shell::Fish => write!(f, "fish"),
            Shell::PowerShell => write!(f, "powershell"),
            Shell::Elvish => write!(f, "elvish"),
        }
    }
}

/// Command-line arguments for the `sentinel completions` command.
#[derive(Args, Debug, Clone)]
pub struct CompletionsArgs {
    /// Target shell to generate completions for
    #[arg(value_name = "SHELL")]
    pub shell: String,
}

/// Command-line arguments for `sentinel audit-limits`.
#[derive(Args, Debug, Clone)]
pub struct AuditLimitsArgs {
    /// Contract address (C...)
    #[arg(value_name = "CONTRACT_ID")]
    pub contract_id: String,

    /// Network name (testnet, mainnet, futurenet, local)
    #[arg(long, default_value = "testnet")]
    pub network: String,

    /// Custom Soroban RPC endpoint URL
    #[arg(long, env = "SOROBAN_RPC_URL")]
    pub rpc_url: Option<String>,

    /// Timeout in seconds for RPC requests (1-120)
    #[arg(long, value_name = "SECONDS")]
    pub rpc_timeout: Option<u64>,

    /// Maximum retry attempts for transient RPC errors (0-10)
    #[arg(long, value_name = "COUNT")]
    pub rpc_max_retries: Option<u32>,

    /// Output format (table, json, text)
    #[arg(long, default_value = "table", value_parser = ["table", "json", "text"])]
    pub format: String,

    /// Additional persistent symbol keys to inspect
    #[arg(long = "key", value_name = "SYMBOL")]
    pub key: Vec<String>,

    /// Additional persistent base64 XDR keys to inspect
    #[arg(long = "key-xdr", value_name = "BASE64_XDR")]
    pub key_xdr: Vec<String>,

    /// Additional persistent key expressions to inspect
    #[arg(long = "key-expr", value_name = "EXPR")]
    pub key_expr: Vec<String>,
}

/// Command-line arguments for `sentinel generate-github-workflow`.
#[derive(Args, Debug, Clone)]
pub struct GenerateWorkflowArgs {
    /// Output directory for scaffolding workflow (defaults to ".")
    #[arg(long = "out-dir", default_value = ".", value_name = "PATH")]
    pub out_dir: std::path::PathBuf,

    /// Cron schedule expression for automated drift scans (defaults to daily "0 0 * * *")
    #[arg(long, default_value = "0 0 * * *", value_name = "EXPR")]
    pub cron: String,

    /// Overwrite existing workflow file
    #[arg(long)]
    pub force: bool,

    /// Target network to audit in CI
    #[arg(long, default_value = "mainnet", value_name = "NETWORK")]
    pub network: String,
}
/// Command-line arguments for `sentinel trend`.
#[derive(Args, Debug, Clone)]
pub struct TrendArgs {
    /// Directory containing historical JSON scan reports
    #[arg(long = "input-dir", value_name = "DIR")]
    pub input_dir: std::path::PathBuf,

    /// Optional contract ID filter (required when input directory contains multiple contracts)
    #[arg(long, value_name = "ID")]
    pub contract: Option<String>,

    /// Warning threshold in ledgers for projecting threshold crossings
    #[arg(long, default_value_t = 120960, value_name = "LEDGERS")]
    pub warning_threshold: u32,

    /// Critical threshold in ledgers for projecting threshold crossings
    #[arg(long, value_name = "LEDGERS")]
    pub critical_threshold: Option<u32>,

    /// Output format: "text" (ASCII table) or "json"
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
                assert!(scan.temp_key.is_empty());
                assert!(scan.temp_key_xdr.is_empty());
                assert_eq!(scan.instance_size_warn_pct, 75);
                assert_eq!(scan.instance_size_critical_pct, 90);
                assert!(scan.rpc_timeout.is_none());
                assert!(scan.rpc_max_retries.is_none());
                assert!(scan.ledger_seconds.is_none());
                assert_eq!(scan.sample_window, DEFAULT_SAMPLE_WINDOW_LEDGERS);
                assert!(scan.baseline.is_none());
                assert!(scan.fail_on_risk.is_none());
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
            "--sample-window",
            "250",
            "--baseline",
            "baseline.json",
            "--fail-on-risk",
            "critical",
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
                assert_eq!(scan.format, "ndjson");
                assert!(scan.no_color);
                assert_eq!(scan.output.as_deref(), Some("report.csv"));
                assert_eq!(scan.threshold, 50000);
                assert_eq!(scan.key, vec!["COUNTER", "ADMIN"]);
                assert_eq!(scan.key_xdr, vec!["AAAAFA=="]);
                assert_eq!(scan.key_expr, vec!["sym:Balance", "vec[sym:Data, u32:1]"]);
                assert_eq!(scan.temp_key, vec!["NONCE", "SESSION"]);
                assert_eq!(scan.temp_key_xdr, vec!["AAAAAw=="]);
                assert_eq!(scan.rpc_timeout, Some(30));
                assert_eq!(scan.rpc_max_retries, Some(5));
                assert_eq!(scan.ledger_seconds, Some(5.5));
                assert_eq!(scan.sample_window, 250);
                assert_eq!(scan.baseline.as_deref(), Some("baseline.json"));
                assert_eq!(scan.fail_on_risk.as_deref(), Some("critical"));
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

    #[test]
    fn test_parse_audit_limits_defaults_and_custom() {
        let contract = "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM";
        let defaults = Cli::parse_from(["sentinel", "audit-limits", contract]);
        match defaults.command {
            Commands::AuditLimits(limits) => {
                assert_eq!(limits.contract_id, contract);
                assert_eq!(limits.network, "testnet");
                assert_eq!(limits.format, "table");
                assert!(limits.key.is_empty());
                assert!(limits.rpc_url.is_none());
            }
            _ => panic!("expected audit-limits command"),
        }

        let custom = Cli::parse_from([
            "sentinel",
            "audit-limits",
            contract,
            "--network",
            "mainnet",
            "--format",
            "json",
            "--key",
            "BALANCE",
            "--key-xdr",
            "AAAAFA==",
            "--key-expr",
            "sym:Data",
            "--rpc-timeout",
            "20",
            "--rpc-max-retries",
            "3",
        ]);
        match custom.command {
            Commands::AuditLimits(limits) => {
                assert_eq!(limits.contract_id, contract);
                assert_eq!(limits.network, "mainnet");
                assert_eq!(limits.format, "json");
                assert_eq!(limits.key, vec!["BALANCE"]);
                assert_eq!(limits.key_xdr, vec!["AAAAFA=="]);
                assert_eq!(limits.key_expr, vec!["sym:Data"]);
                assert_eq!(limits.rpc_timeout, Some(20));
                assert_eq!(limits.rpc_max_retries, Some(3));
            }
            _ => panic!("expected audit-limits command"),
        }
    }

    #[test]
    fn test_parse_generate_workflow_defaults_and_custom() {
        let defaults = Cli::parse_from(["sentinel", "generate-github-workflow"]);
        match defaults.command {
            Commands::GenerateGithubWorkflow(gen) => {
                assert_eq!(gen.out_dir, std::path::PathBuf::from("."));
                assert_eq!(gen.cron, "0 0 * * *");
                assert_eq!(gen.network, "mainnet");
                assert!(!gen.force);
            }
            _ => panic!("expected generate-github-workflow command"),
        }

        let custom = Cli::parse_from([
            "sentinel",
            "generate-github-workflow",
            "--out-dir",
            "custom/dir",
            "--cron",
            "0 12 * * *",
            "--network",
            "testnet",
            "--force",
        ]);
        match custom.command {
            Commands::GenerateGithubWorkflow(gen) => {
                assert_eq!(gen.out_dir, std::path::PathBuf::from("custom/dir"));
                assert_eq!(gen.cron, "0 12 * * *");
                assert_eq!(gen.network, "testnet");
                assert!(gen.force);
            }
            _ => panic!("expected generate-github-workflow command"),
        }
    }

    #[test]
    fn test_parse_trend_defaults_and_flags() {
        let defaults = Cli::parse_from(["sentinel", "trend", "--input-dir", "./scans"]);
        match defaults.command {
            Commands::Trend(trend) => {
                assert_eq!(trend.input_dir, std::path::PathBuf::from("./scans"));
                assert!(trend.contract.is_none());
                assert_eq!(trend.warning_threshold, 120960);
                assert!(trend.critical_threshold.is_none());
                assert_eq!(trend.format, "text");
            }
            _ => panic!("expected trend command"),
        }

        let contract = "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM";
        let custom = Cli::parse_from([
            "sentinel",
            "trend",
            "--input-dir",
            "/tmp/history",
            "--contract",
            contract,
            "--warning-threshold",
            "100000",
            "--critical-threshold",
            "25000",
            "--format",
            "json",
        ]);
        match custom.command {
            Commands::Trend(trend) => {
                assert_eq!(trend.input_dir, std::path::PathBuf::from("/tmp/history"));
                assert_eq!(trend.contract.as_deref(), Some(contract));
                assert_eq!(trend.warning_threshold, 100000);
                assert_eq!(trend.critical_threshold, Some(25000));
                assert_eq!(trend.format, "json");
            }
            _ => panic!("expected trend command"),
        }
    }
}

#[derive(Args, Debug)]
pub struct ExportOpenapiArgs {
    #[arg(long, default_value = "json", value_parser = ["json", "yaml"])]
    pub format: String,
}

#[derive(Args, Debug)]
pub struct HealthMatrixArgs {
    #[arg(long, value_name = "PATH")]
    pub manifest: std::path::PathBuf,
    /// Show only contracts scoring below this minimum.
    #[arg(long, value_parser = clap::value_parser!(u8).range(0..=100))]
    pub min_score: Option<u8>,
    /// Persistent-key count consuming all 20 count-headroom points.
    #[arg(long, default_value_t = 100, value_parser = clap::value_parser!(u32).range(1..))]
    pub entry_budget: u32,
    /// Largest-entry byte size consuming all 20 storage-headroom points.
    #[arg(long, default_value_t = 65536, value_parser = clap::value_parser!(u64).range(1..))]
    pub storage_budget_bytes: u64,
    /// Supply a planning rent rate together with its denominator.
    #[arg(long, requires = "rent_denominator")]
    pub rent_fee_per_kib: Option<u64>,
    #[arg(long, requires = "rent_fee_per_kib", value_parser = clap::value_parser!(u64).range(1..))]
    pub rent_denominator: Option<u64>,
}
