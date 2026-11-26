//! Execution logic for the `sentinel scan` CLI command.

use std::fs;
#[cfg(any(feature = "tui", feature = "parquet"))]
use std::io::IsTerminal;
#[cfg(feature = "parquet")]
use std::io::Write;
use std::path::Path;
use std::time::Duration;

use sentinel_core::adapters::{validate_rpc_url, SorobanRpcClient};
use sentinel_core::domain::compare::{compare_reports, BaselineSnapshot, MAX_BASELINE_FILE_BYTES};
use sentinel_core::domain::{
    build_recommendation_plan, shell_quote, validate_sample_window, RecommendationAction,
    RiskLevel, ThresholdConfig, DEFAULT_CRITICAL_LEDGERS,
};
use sentinel_core::error::SentinelError;
use sentinel_core::usecases::{fetch_network_limits, ScanContractUseCase, ScanRequest, TTLReport};
use sentinel_core::{AlertKind, AlertMinLevel, AlertNotifier, WebhookAlertNotifier};

use crate::args::ScanArgs;
#[cfg(feature = "parquet")]
use crate::formatters::render_parquet;
use crate::formatters::{
    render_csv, render_html, render_json_with_diff, render_markdown, render_ndjson,
    render_pushgateway, render_sarif, render_sonarqube, render_text_with_diff,
    render_text_with_diff_and_options,
};

/// Exit code: All evaluated storage entries have status OK.
pub const EXIT_OK: i32 = 0;
/// Exit code: One or more entries have status WARN.
pub const EXIT_WARN: i32 = 1;
/// Exit code: One or more entries have status CRITICAL or ARCHIVED.
pub const EXIT_CRITICAL_OR_ARCHIVED: i32 = 2;
/// Exit code: Invalid command-line arguments or address format.
pub const EXIT_INVALID_ARGS: i32 = 10;
/// Exit code: RPC communication, timeout, or network failure.
pub const EXIT_RPC_FAILURE: i32 = 11;

fn resolve_scan_exit_code(
    status: RiskLevel,
    fail_on_risk: bool,
    fail_on_regression: bool,
    diff_has_regression: bool,
) -> i32 {
    if fail_on_risk {
        if matches!(status, RiskLevel::Critical | RiskLevel::Archived)
            || (fail_on_regression && diff_has_regression)
        {
            EXIT_CRITICAL_OR_ARCHIVED
        } else {
            EXIT_OK
        }
    } else if fail_on_regression && diff_has_regression {
        EXIT_CRITICAL_OR_ARCHIVED
    } else {
        match status {
            RiskLevel::Ok => EXIT_OK,
            RiskLevel::Warn => EXIT_WARN,
            RiskLevel::Critical | RiskLevel::Archived => EXIT_CRITICAL_OR_ARCHIVED,
        }
    }
}

/// Resolves the appropriate RPC endpoint URL from network preset or command override.
pub fn resolve_rpc_url(network: &str, rpc_url_override: Option<&str>) -> Result<String, String> {
    if let Some(url) = rpc_url_override {
        validate_rpc_url(url).map_err(|e| e.to_string())?;
        return Ok(url.to_string());
    }

    match network.to_lowercase().as_str() {
        "testnet" => Ok("https://soroban-testnet.stellar.org".to_string()),
        "mainnet" => Ok("https://soroban-rpc.stellar.org".to_string()),
        "futurenet" => Ok("https://rpc-futurenet.stellar.org".to_string()),
        "local" => Ok("http://localhost:8000/soroban/rpc".to_string()),
        other => Err(format!(
            "Unknown network '{}'. Use --rpc-url to specify a custom endpoint.",
            other
        )),
    }
}

/// Resolves and validates a sound threshold configuration.
pub fn resolve_threshold_config(
    warning_threshold: u32,
    critical_threshold_opt: Option<u32>,
) -> Result<ThresholdConfig, String> {
    if warning_threshold == 0 {
        return Err("Warning threshold must be greater than 0".to_string());
    }

    let critical_ledgers = match critical_threshold_opt {
        Some(crit) => crit,
        None => {
            if warning_threshold <= DEFAULT_CRITICAL_LEDGERS {
                std::cmp::max(1, warning_threshold / 3)
            } else {
                DEFAULT_CRITICAL_LEDGERS
            }
        }
    };

    ThresholdConfig::try_new(warning_threshold, critical_ledgers).map_err(|e| e.to_string())
}

/// Loads and validates a baseline JSON file against the target contract and network before running queries.
pub fn load_and_validate_baseline(
    baseline_path_str: &str,
    target_contract_id: &str,
    target_network: &str,
) -> Result<BaselineSnapshot, String> {
    let path = Path::new(baseline_path_str);
    if !path.exists() {
        return Err(format!("Baseline file '{}' not found", baseline_path_str));
    }

    let metadata = fs::metadata(path).map_err(|e| {
        format!(
            "Failed to read metadata for baseline file '{}': {}",
            baseline_path_str, e
        )
    })?;

    if metadata.len() > MAX_BASELINE_FILE_BYTES {
        return Err(format!(
            "Baseline file '{}' exceeds maximum allowed size of {} bytes (size: {} bytes)",
            baseline_path_str,
            MAX_BASELINE_FILE_BYTES,
            metadata.len()
        ));
    }

    let contents = fs::read_to_string(path).map_err(|e| {
        format!(
            "Failed to read baseline file '{}': {}",
            baseline_path_str, e
        )
    })?;

    let snapshot: BaselineSnapshot = serde_json::from_str(&contents)
        .map_err(|e| format!("Malformed baseline JSON in '{}': {}", baseline_path_str, e))?;

    if snapshot.contract_id != target_contract_id {
        return Err(format!(
            "Baseline contract ID '{}' does not match target contract ID '{}'",
            snapshot.contract_id, target_contract_id
        ));
    }

    if !snapshot.network.eq_ignore_ascii_case(target_network) {
        return Err(format!(
            "Baseline network '{}' does not match target network '{}'",
            snapshot.network, target_network
        ));
    }

    Ok(snapshot)
}

/// Executes the contract scan command and returns the appropriate UNIX exit code.
pub(super) fn prepare_scan(
    args: &ScanArgs,
) -> Result<(ScanContractUseCase<SorobanRpcClient>, ScanRequest), String> {
    if args
        .rpc_timeout
        .is_some_and(|timeout| !(1..=120).contains(&timeout))
    {
        return Err(format!(
            "--rpc-timeout must be between 1 and 120 seconds (got {})",
            args.rpc_timeout.unwrap_or_default()
        ));
    }
    if args.rpc_max_retries.is_some_and(|retries| retries > 10) {
        return Err(format!(
            "--rpc-max-retries must be between 0 and 10 (got {})",
            args.rpc_max_retries.unwrap_or_default()
        ));
    }
    if args
        .ledger_seconds
        .is_some_and(|seconds| !(1.0..=60.0).contains(&seconds))
    {
        return Err(format!(
            "--ledger-seconds must be between 1.0 and 60.0 (got {})",
            args.ledger_seconds.unwrap_or_default()
        ));
    }
    let url = resolve_rpc_url(&args.network, args.rpc_url.as_deref())?;
    let thresholds = resolve_threshold_config(args.threshold, args.critical_threshold)?;
    let mut client = SorobanRpcClient::new_with_timeout(
        url,
        Duration::from_secs(args.rpc_timeout.unwrap_or(10)),
    );
    if let Some(retries) = args.rpc_max_retries {
        let mut policy = client.retry_policy().clone();
        policy.max_retries = retries;
        client = client.with_retry_policy(policy);
    }
    let request = ScanRequest::new(&args.contract_id)
        .with_network(&args.network)
        .with_threshold_config(thresholds)
        .with_persistent_symbols(args.key.clone())
        .with_persistent_xdr_keys(args.key_xdr.clone())
        .with_persistent_key_exprs(args.key_expr.clone())
        .with_ledger_seconds(args.ledger_seconds);
    Ok((ScanContractUseCase::new(client), request))
}

/// Verifies that `--output` points to a writable file location without
/// truncating an existing file. The actual write still happens after a
/// successful scan and overwrites/truncates the destination.
fn validate_output_path(path: &str) -> Result<(), String> {
    let path = Path::new(path);
    if path.as_os_str().is_empty() {
        return Err("--output path must not be empty".to_string());
    }

    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() && !parent.is_dir() {
            return Err(format!(
                "--output parent directory '{}' does not exist",
                parent.display()
            ));
        }
    }

    if path.is_dir() {
        return Err(format!("--output path '{}' is a directory", path.display()));
    }

    if path.exists() {
        fs::OpenOptions::new()
            .write(true)
            .open(path)
            .map_err(|e| format!("--output path '{}' is not writable: {}", path.display(), e))?;
    } else {
        let file = fs::File::create(path)
            .map_err(|e| format!("--output path '{}' is not writable: {}", path.display(), e))?;
        drop(file);
        let _ = fs::remove_file(path);
    }

    Ok(())
}

/// Refuses to write binary Parquet bytes to an interactive terminal.
#[cfg(feature = "parquet")]
fn validate_parquet_stdout(has_output: bool, stdout_is_terminal: bool) -> Result<(), &'static str> {
    if !has_output && stdout_is_terminal {
        Err("--format parquet writes binary data; use --output <FILE> or redirect stdout to a file")
    } else {
        Ok(())
    }
}

/// Renders and writes Parquet bytes to `--output` or to piped stdout.
fn emit_parquet(report: &TTLReport, path: Option<&str>) -> Result<(), String> {
    #[cfg(feature = "parquet")]
    {
        let bytes = render_parquet(report).map_err(|err| err.to_string())?;
        match path {
            Some(path) => fs::write(path, &bytes)
                .map_err(|err| format!("failed to write --output '{}': {}", path, err)),
            None => std::io::stdout()
                .lock()
                .write_all(&bytes)
                .map_err(|err| format!("failed to write Parquet output to stdout: {}", err)),
        }
    }
    #[cfg(not(feature = "parquet"))]
    {
        // Unreachable: run_scan rejects --format parquet before any RPC work
        // when the feature is disabled.
        let _ = (report, path);
        Ok(())
    }
}

fn render_recommendation_text(
    plan: &sentinel_core::domain::RecommendationPlan,
    contract_id: &str,
    rpc_url: &str,
    network: &str,
) -> String {
    let mut lines = vec!["Recommendations:".to_string()];
    for recommendation in &plan.recommendations {
        match &recommendation.action {
            RecommendationAction::Extend {
                ledgers_to_extend,
                extend_to,
            } => {
                if recommendation.durability == sentinel_core::domain::Durability::Persistent {
                    let raw_key = recommendation
                        .key
                        .strip_prefix("persistent:")
                        .unwrap_or(&recommendation.key);
                    lines.push(format!(
                        "  stellar contract extend --id {} --key {} --durability persistent --ledgers-to-extend {} --rpc-url {} --network {}  # target ledger {}",
                        shell_quote(contract_id),
                        shell_quote(raw_key),
                        ledgers_to_extend,
                        shell_quote(rpc_url),
                        shell_quote(network),
                        extend_to
                    ));
                } else if recommendation.durability == sentinel_core::domain::Durability::Instance {
                    lines.push(format!(
                        "  stellar contract extend --id {} --durability persistent --ledgers-to-extend {} --rpc-url {} --network {}  # contract instance; target ledger {}",
                        shell_quote(contract_id),
                        ledgers_to_extend,
                        shell_quote(rpc_url),
                        shell_quote(network),
                        extend_to
                    ));
                }
            }
            RecommendationAction::Restore => {
                lines.push(format!("  RESTORE {}", recommendation.key));
            }
            RecommendationAction::Unrecoverable => {
                lines.push(format!("  UNRECOVERABLE {}", recommendation.key));
            }
        }
    }
    if !plan.network_limit_checked {
        lines.push(
            "  Note: network max-entry-TTL was unavailable; a conservative default was used and was not network-validated."
                .to_string(),
        );
    }
    lines.join("\n")
}

fn inject_recommendations(
    output: &str,
    plan: &sentinel_core::domain::RecommendationPlan,
) -> Result<String, SentinelError> {
    let mut value: serde_json::Value = serde_json::from_str(output)
        .map_err(|error| SentinelError::SerializationError(error.to_string()))?;
    value["recommendations"] = serde_json::to_value(&plan.recommendations)
        .map_err(|error| SentinelError::SerializationError(error.to_string()))?;
    serde_json::to_string_pretty(&value)
        .map_err(|error| SentinelError::SerializationError(error.to_string()))
}

pub async fn run_scan(mut args: ScanArgs) -> i32 {
    if let Err(code) = apply_alias(&mut args) {
        return code;
    }

    #[cfg(not(feature = "tui"))]
    if args.tui {
        eprintln!(
            "Error: --tui requires the CLI to be built with the `tui` feature (rebuild with `cargo install --path crates/sentinel-cli --features tui` or `cargo build -p sentinel-cli --features tui`)"
        );
        return EXIT_INVALID_ARGS;
    }

    #[cfg(feature = "tui")]
    {
        if args.tui && !std::io::stdout().is_terminal() {
            eprintln!("Error: --tui requires an interactive TTY, but stdout is not a terminal");
            return EXIT_INVALID_ARGS;
        }
        if args.tui && args.baseline.is_some() {
            eprintln!("Error: --tui cannot be combined with --baseline");
            return EXIT_INVALID_ARGS;
        }
    }

    let format = args.format.to_lowercase();

    if args.snapshot.is_some()
        && (args.rpc_url.is_some() || args.rpc_timeout.is_some() || args.rpc_max_retries.is_some())
    {
        eprintln!("Error: --snapshot cannot be combined with RPC URL or RPC retry/timeout flags");
        return EXIT_INVALID_ARGS;
    }
    if args.snapshot.is_some() && args.baseline.is_some() {
        eprintln!("Error: --snapshot cannot be combined with --baseline");
        return EXIT_INVALID_ARGS;
    }
    if args.snapshot.is_some() && (format == "csv" || format == "parquet") {
        eprintln!("Error: --snapshot is not supported with --format csv or --format parquet");
        return EXIT_INVALID_ARGS;
    }
    if args.recommend && !matches!(format.as_str(), "text" | "json") {
        eprintln!("Error: --recommend is supported with --format text or json");
        return EXIT_INVALID_ARGS;
    }
    if args.recommend && args.target_days == 0 {
        eprintln!("Error: --target-days must be greater than 0");
        return EXIT_INVALID_ARGS;
    }

    #[cfg(not(feature = "parquet"))]
    if format == "parquet" {
        eprintln!(
            "Error: this binary was built without Parquet support; rebuild with --features parquet"
        );
        return EXIT_INVALID_ARGS;
    }

    #[cfg(feature = "parquet")]
    if format == "parquet" {
        if let Err(err) =
            validate_parquet_stdout(args.output.is_some(), std::io::stdout().is_terminal())
        {
            eprintln!("Error: {}", err);
            return EXIT_INVALID_ARGS;
        }
    }

    if format == "csv" && args.baseline.is_some() {
        eprintln!("Error: --baseline is not supported with --format csv");
        return EXIT_INVALID_ARGS;
    }

    if format == "parquet" && args.baseline.is_some() {
        eprintln!("Error: --baseline is not supported with --format parquet");
        return EXIT_INVALID_ARGS;
    }

    if let Some(ref path) = args.output {
        if let Err(err) = validate_output_path(path) {
            eprintln!("Error: {}", err);
            return EXIT_INVALID_ARGS;
        }
    }

    // 1. Validate argument bounds before making any RPC calls
    if let Some(timeout) = args.rpc_timeout {
        if !(1..=120).contains(&timeout) {
            eprintln!(
                "Error: --rpc-timeout must be between 1 and 120 seconds (got {})",
                timeout
            );
            return EXIT_INVALID_ARGS;
        }
    }

    if let Some(retries) = args.rpc_max_retries {
        if retries > 10 {
            eprintln!(
                "Error: --rpc-max-retries must be between 0 and 10 (got {})",
                retries
            );
            return EXIT_INVALID_ARGS;
        }
    }

    if let Some(ledger_seconds) = args.ledger_seconds {
        if !(1.0..=60.0).contains(&ledger_seconds) {
            eprintln!(
                "Error: --ledger-seconds must be between 1.0 and 60.0 (got {})",
                ledger_seconds
            );
            return EXIT_INVALID_ARGS;
        }
    }

    if let Err(error) = validate_sample_window(args.sample_window) {
        eprintln!("Error: --sample-window {}", error);
        return EXIT_INVALID_ARGS;
    }

    let baseline_snapshot = if let Some(ref path) = args.baseline {
        match load_and_validate_baseline(path, &args.contract_id, &args.network) {
            Ok(s) => Some(s),
            Err(err) => {
                eprintln!("Error: {}", err);
                return EXIT_INVALID_ARGS;
            }
        }
    } else {
        None
    };

    let threshold_config = match resolve_threshold_config(args.threshold, args.critical_threshold) {
        Ok(cfg) => cfg,
        Err(err) => {
            eprintln!("Error: {}", err);
            return EXIT_INVALID_ARGS;
        }
    };

    let mut request = ScanRequest::new(&args.contract_id)
        .with_network(&args.network)
        .with_threshold_config(threshold_config)
        .with_persistent_symbols(args.key)
        .with_persistent_xdr_keys(args.key_xdr)
        .with_persistent_key_exprs(args.key_expr)
        .with_sample_window(args.sample_window);

    if let Some(ledger_seconds) = args.ledger_seconds {
        request = request.with_ledger_seconds(Some(ledger_seconds));
    }

    let (report, network_reader, rpc_url) = if let Some(snapshot_path) = args.snapshot.as_deref() {
        let replay = match sentinel_core::adapters::ReplayReader::replay(
            snapshot_path,
            network_passphrase(&args.network),
        ) {
            Ok(reader) => reader,
            Err(error) => {
                eprintln!("Error: {error}");
                return EXIT_INVALID_ARGS;
            }
        };
        let rpc_url = match resolve_rpc_url(&args.network, None) {
            Ok(url) => url,
            Err(err) if args.recommend => {
                eprintln!("Error: {}", err);
                return EXIT_INVALID_ARGS;
            }
            Err(_) => String::new(),
        };
        match ScanContractUseCase::new(replay).execute(request).await {
            Ok(report) => (report, None, rpc_url),
            Err(error) => {
                eprintln!("Error: {error}");
                return EXIT_INVALID_ARGS;
            }
        }
    } else {
        let rpc_url = match resolve_rpc_url(&args.network, args.rpc_url.as_deref()) {
            Ok(url) => url,
            Err(err) => {
                eprintln!("Error: {}", err);
                return EXIT_INVALID_ARGS;
            }
        };
        let rpc_timeout = Duration::from_secs(args.rpc_timeout.unwrap_or(10));
        let mut rpc_client = SorobanRpcClient::new_with_timeout(rpc_url.clone(), rpc_timeout);
        if let Some(retries) = args.rpc_max_retries {
            let mut policy = rpc_client.retry_policy().clone();
            policy.max_retries = retries;
            rpc_client = rpc_client.with_retry_policy(policy);
        }
        let network_reader = rpc_client.clone();
        match ScanContractUseCase::new(rpc_client).execute(request).await {
            Ok(report) => (report, Some(network_reader), rpc_url),
            Err(err) => {
                eprintln!("Error: {err}");
                return match err {
                    SentinelError::InvalidContractAddress(_)
                    | SentinelError::InvalidXdrKey(_)
                    | SentinelError::InvalidThresholdConfig(_)
                    | SentinelError::InvalidRpcUrl(_) => EXIT_INVALID_ARGS,
                    SentinelError::RpcConnectionTimeout(_)
                    | SentinelError::RpcResponseError { .. }
                    | SentinelError::SerializationError(_) => EXIT_RPC_FAILURE,
                    _ => EXIT_INVALID_ARGS,
                };
            }
        }
    };

    if let Some(webhook) = args.alert_webhook.as_deref() {
        let notifier = match WebhookAlertNotifier::new(webhook) {
            Ok(notifier) => notifier,
            Err(error) => {
                eprintln!("Error: {error}");
                return EXIT_INVALID_ARGS;
            }
        };
        let kind = match args.alert_kind.as_str() {
            "slack" => AlertKind::Slack,
            "discord" => AlertKind::Discord,
            _ => return EXIT_INVALID_ARGS,
        };
        let min_level = match args.alert_min_level.as_str() {
            "warn" => AlertMinLevel::Warn,
            "critical" => AlertMinLevel::Critical,
            _ => return EXIT_INVALID_ARGS,
        };
        if let Err(_error) = notifier.notify(&report, kind, min_level).await {
            eprintln!("Warning: alert delivery failed");
        }
    }

    #[cfg(feature = "tui")]
    if args.tui {
        return crate::commands::tui::run_tui(report, threshold_config, &args.network);
    }
    crate::telemetry::record_scan_metrics(&report);

    let baseline_diff = baseline_snapshot
        .as_ref()
        .map(|b| compare_reports(&report, b));

    let recommendation_plan = if args.recommend {
        let max_ttl = match network_reader.as_ref() {
            Some(reader) => fetch_network_limits(reader)
                .await
                .ok()
                .map(|limits| limits.max_entry_ttl),
            None => None,
        };
        Some(build_recommendation_plan(
            &report.entries,
            report.latest_ledger,
            args.target_days,
            report.ledger_velocity_seconds,
            max_ttl,
        ))
    } else {
        None
    };

    let diff_has_regression = baseline_diff
        .as_ref()
        .map(|d| d.summary.has_regression)
        .unwrap_or(false);
    let final_exit_code = resolve_scan_exit_code(
        report.overall_status,
        args.fail_on_risk.as_deref() == Some("critical"),
        args.fail_on_regression,
        diff_has_regression,
    );

    if format == "parquet" {
        return match emit_parquet(&report, args.output.as_deref()) {
            Ok(()) => final_exit_code,
            Err(err) => {
                eprintln!("Error: {}", err);
                EXIT_INVALID_ARGS
            }
        };
    }

    let output_result = match format.as_str() {
        "text" => {
            let no_color = args.no_color || args.output.is_some();
            let mut output =
                render_text_with_diff_and_options(&report, baseline_diff.as_ref(), no_color);
            if let Some(plan) = recommendation_plan.as_ref() {
                output.push('\n');
                output.push_str(&render_recommendation_text(
                    plan,
                    &args.contract_id,
                    &rpc_url,
                    &args.network,
                ));
            }
            Ok(output)
        }
        "json" => match render_json_with_diff(&report, baseline_diff.clone()) {
            Ok(rendered) => match recommendation_plan.as_ref() {
                Some(plan) => inject_recommendations(&rendered, plan),
                None => Ok(rendered),
            },
            Err(error) => Err(error),
        },
        "ndjson" => render_ndjson(&report),
        "csv" => render_csv(&report),
        "html" => Ok(render_html(&report)),
        "pushgateway" => Ok(render_pushgateway(&report)),
        "sarif" => render_sarif(&report),
        "sonarqube" => render_sonarqube(&report),
        "markdown" => Ok(render_markdown(&report)),
        other => {
            let supported = if cfg!(feature = "parquet") {
                "text, json, ndjson, csv, parquet, sarif, markdown, html, pushgateway, sonarqube"
            } else {
                "text, json, ndjson, csv, sarif, markdown, html, pushgateway, sonarqube"
            };
            eprintln!(
                "Error: Unsupported format '{}'. Supported formats: {}",
                other, supported
            );
            return EXIT_INVALID_ARGS;
        }
    };

    match output_result {
        Ok(output) => {
            if let Some(path) = args.output.as_deref() {
                let contents = if output.is_empty() {
                    String::new()
                } else {
                    format!("{}\n", output.trim_end_matches('\n'))
                };
                if let Err(err) = fs::write(path, contents) {
                    eprintln!("Error: failed to write --output '{}': {}", path, err);
                    return EXIT_INVALID_ARGS;
                }
            } else if matches!(format.as_str(), "csv" | "pushgateway") {
                // These formats already terminate their final line.
                println!("{}", output.trim_end_matches('\n'));
            } else if format == "ndjson" {
                if !output.is_empty() {
                    println!("{}", output.trim_end_matches('\n'));
                }
            } else {
                println!("{}", output);
            }

            final_exit_code
        }
        Err(err) => {
            eprintln!("Error: {}", err);
            EXIT_INVALID_ARGS
        }
    }
}

/// Returns the network passphrase used by snapshot replay compatibility checks.
pub fn network_passphrase(network: &str) -> &'static str {
    match network.to_lowercase().as_str() {
        "testnet" => "Test SDF Network ; September 2015",
        "mainnet" => "Public Global Stellar Network ; September 2015",
        "futurenet" => "Test SDF Future Network ; October 2022",
        "local" => "Standalone Network ; February 2017",
        _ => "",
    }
}

/// Resolves a contract alias into a `C...` strkey before the scan starts.
///
/// A valid strkey passes through untouched. Aliases are read from the TOML file
/// named by `SENTINEL_ALIASES`, or from `./sentinel-aliases.toml` when present:
///
/// ```toml
/// [aliases]
/// treasury = "C..."
/// ```
///
/// Unknown targets without a close match fall through to the normal
/// "Invalid contract address" handling.
fn apply_alias(args: &mut ScanArgs) -> Result<(), i32> {
    use sentinel_core::domain::alias::{AliasError, AliasRegistry};

    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct AliasFile {
        #[serde(default)]
        aliases: std::collections::BTreeMap<String, String>,
    }

    let target = args.contract_id.trim().to_string();
    if stellar_strkey::Contract::from_string(&target).is_ok() {
        return Ok(());
    }

    let path = match std::env::var_os("SENTINEL_ALIASES").filter(|value| !value.is_empty()) {
        Some(value) => std::path::PathBuf::from(value),
        None => {
            let default = std::path::PathBuf::from("sentinel-aliases.toml");
            if !default.is_file() {
                return Ok(());
            }
            default
        }
    };

    let text = std::fs::read_to_string(&path).map_err(|error| {
        eprintln!("Error: cannot read alias file {}: {error}", path.display());
        EXIT_INVALID_ARGS
    })?;
    let file: AliasFile = toml::from_str(&text).map_err(|error| {
        eprintln!("Error: invalid alias file {}: {error}", path.display());
        EXIT_INVALID_ARGS
    })?;
    let registry = AliasRegistry::from_pairs(file.aliases).map_err(|error| {
        eprintln!("Error: invalid alias file {}: {error}", path.display());
        EXIT_INVALID_ARGS
    })?;

    match registry.resolve(&target) {
        Ok(contract_id) => {
            args.contract_id = contract_id;
            Ok(())
        }
        Err(error @ AliasError::UnknownWithSuggestion { .. }) => {
            eprintln!("Error: Invalid contract address: {error}");
            Err(EXIT_INVALID_ARGS)
        }
        Err(_) => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fail_on_critical_risk_only_fails_at_critical_or_archived() {
        assert_eq!(
            resolve_scan_exit_code(RiskLevel::Ok, true, false, false),
            EXIT_OK
        );
        assert_eq!(
            resolve_scan_exit_code(RiskLevel::Warn, true, false, false),
            EXIT_OK
        );
        assert_eq!(
            resolve_scan_exit_code(RiskLevel::Critical, true, false, false),
            EXIT_CRITICAL_OR_ARCHIVED
        );
        assert_eq!(
            resolve_scan_exit_code(RiskLevel::Archived, true, false, false),
            EXIT_CRITICAL_OR_ARCHIVED
        );
        assert_eq!(
            resolve_scan_exit_code(RiskLevel::Warn, false, false, false),
            EXIT_WARN
        );
    }

    #[cfg(feature = "parquet")]
    #[test]
    fn test_validate_parquet_stdout() {
        assert!(validate_parquet_stdout(true, true).is_ok());
        assert!(validate_parquet_stdout(false, false).is_ok());
        assert!(validate_parquet_stdout(true, false).is_ok());
        assert_eq!(
            validate_parquet_stdout(false, true),
            Err("--format parquet writes binary data; use --output <FILE> or redirect stdout to a file")
        );
    }

    #[test]
    fn test_resolve_rpc_url_presets() {
        assert_eq!(
            resolve_rpc_url("testnet", None).unwrap(),
            "https://soroban-testnet.stellar.org"
        );
        assert_eq!(
            resolve_rpc_url("mainnet", None).unwrap(),
            "https://soroban-rpc.stellar.org"
        );
        assert_eq!(
            resolve_rpc_url("futurenet", None).unwrap(),
            "https://rpc-futurenet.stellar.org"
        );
        assert_eq!(
            resolve_rpc_url("local", None).unwrap(),
            "http://localhost:8000/soroban/rpc"
        );
    }

    #[test]
    fn test_resolve_rpc_url_override() {
        assert_eq!(
            resolve_rpc_url("testnet", Some("https://custom.rpc.com")).unwrap(),
            "https://custom.rpc.com"
        );
    }

    #[test]
    fn test_resolve_rpc_url_unknown_network() {
        assert!(resolve_rpc_url("unknown_net", None).is_err());
    }

    #[test]
    fn test_resolve_threshold_config_behavior() {
        // 1. Defaults: warning > DEFAULT_CRITICAL_LEDGERS
        let cfg = resolve_threshold_config(120960, None).unwrap();
        assert_eq!(cfg.warning_ledgers, 120960);
        assert_eq!(cfg.critical_ledgers, DEFAULT_CRITICAL_LEDGERS);

        // 2. Custom warning <= DEFAULT_CRITICAL_LEDGERS scales critical to 1/3 (preventing bypass)
        let cfg = resolve_threshold_config(10000, None).unwrap();
        assert_eq!(cfg.warning_ledgers, 10000);
        assert_eq!(cfg.critical_ledgers, 3333);
        assert!(cfg.critical_ledgers < cfg.warning_ledgers);

        // 3. Custom warning and explicit critical
        let cfg = resolve_threshold_config(50000, Some(10000)).unwrap();
        assert_eq!(cfg.warning_ledgers, 50000);
        assert_eq!(cfg.critical_ledgers, 10000);

        // 4. Contradictory: critical >= warning returns error
        assert!(resolve_threshold_config(50000, Some(50000)).is_err());
        assert!(resolve_threshold_config(50000, Some(60000)).is_err());

        // 5. Invalid: warning == 0 or critical == 0
        assert!(resolve_threshold_config(0, None).is_err());
        assert!(resolve_threshold_config(100, Some(0)).is_err());
    }

    #[test]
    fn test_load_and_validate_baseline_validation() {
        let temp_dir = std::env::temp_dir();
        let valid_baseline_file = temp_dir.join("test_valid_baseline.json");
        let valid_json = r#"{
            "contract_id": "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM",
            "network": "testnet",
            "latest_ledger": 5400000,
            "entries": []
        }"#;
        fs::write(&valid_baseline_file, valid_json).unwrap();

        // Valid load
        let loaded = load_and_validate_baseline(
            valid_baseline_file.to_str().unwrap(),
            "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM",
            "testnet",
        );
        assert!(loaded.is_ok());

        // Mismatched contract ID rejected
        let err_contract = load_and_validate_baseline(
            valid_baseline_file.to_str().unwrap(),
            "CDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDD2KM",
            "testnet",
        );
        assert!(err_contract.is_err());
        assert!(err_contract.unwrap_err().contains("contract ID"));

        // Mismatched network rejected
        let err_net = load_and_validate_baseline(
            valid_baseline_file.to_str().unwrap(),
            "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM",
            "mainnet",
        );
        assert!(err_net.is_err());
        assert!(err_net.unwrap_err().contains("network"));

        // Non-existent file rejected
        assert!(load_and_validate_baseline("non_existent_file.json", "CAAA", "testnet").is_err());

        // Malformed JSON rejected
        let malformed_file = temp_dir.join("test_malformed_baseline.json");
        fs::write(&malformed_file, "NOT VALID JSON").unwrap();
        let err_malformed = load_and_validate_baseline(
            malformed_file.to_str().unwrap(),
            "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM",
            "testnet",
        );
        assert!(err_malformed.is_err());
        assert!(err_malformed
            .unwrap_err()
            .contains("Malformed baseline JSON"));

        let _ = fs::remove_file(valid_baseline_file);
        let _ = fs::remove_file(malformed_file);
    }
}
