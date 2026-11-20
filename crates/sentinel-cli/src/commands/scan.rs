//! Execution logic for the `sentinel scan` CLI command.

use std::fs;
use std::path::Path;
use std::time::Duration;

use sentinel_core::adapters::{validate_rpc_url, SorobanRpcClient};
use sentinel_core::domain::compare::{compare_reports, BaselineSnapshot, MAX_BASELINE_FILE_BYTES};
use sentinel_core::domain::{RiskLevel, ThresholdConfig, DEFAULT_CRITICAL_LEDGERS};
use sentinel_core::error::SentinelError;
use sentinel_core::usecases::{ScanContractUseCase, ScanRequest};

use crate::args::{OutputFormat, ScanArgs};
use crate::formatters::{
    render_json_with_diff, render_markdown, render_prometheus, render_sarif, render_text_with_diff,
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
pub async fn run_scan(args: ScanArgs) -> i32 {
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

    let rpc_url = match resolve_rpc_url(&args.network, args.rpc_url.as_deref()) {
        Ok(url) => url,
        Err(err) => {
            eprintln!("Error: {}", err);
            return EXIT_INVALID_ARGS;
        }
    };

    let threshold_config = match resolve_threshold_config(args.threshold, args.critical_threshold) {
        Ok(cfg) => cfg,
        Err(err) => {
            eprintln!("Error: {}", err);
            return EXIT_INVALID_ARGS;
        }
    };

    let rpc_timeout = Duration::from_secs(args.rpc_timeout.unwrap_or(10));
    let mut rpc_client = SorobanRpcClient::new_with_timeout(rpc_url, rpc_timeout);
    if let Some(retries) = args.rpc_max_retries {
        let mut policy = rpc_client.retry_policy().clone();
        policy.max_retries = retries;
        rpc_client = rpc_client.with_retry_policy(policy);
    }

    let usecase = ScanContractUseCase::new(rpc_client);

    let mut request = ScanRequest::new(&args.contract_id)
        .with_network(&args.network)
        .with_threshold_config(threshold_config)
        .with_persistent_symbols(args.key)
        .with_persistent_xdr_keys(args.key_xdr)
        .with_persistent_key_exprs(args.key_expr)
        .with_temporary_symbols(args.temp_key)
        .with_temporary_xdr_keys(args.temp_key_xdr);

    if let Some(ls) = args.ledger_seconds {
        request = request.with_ledger_seconds(Some(ls));
    }

    let report = match usecase.execute(request).await {
        Ok(r) => r,
        Err(err) => {
            eprintln!("Error: {}", err);
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
    };

    let baseline_diff = baseline_snapshot
        .as_ref()
        .map(|b| compare_reports(&report, b));

    let output_result = match args.format {
        OutputFormat::Text => Ok(render_text_with_diff(&report, baseline_diff.as_ref())),
        OutputFormat::Json => render_json_with_diff(&report, baseline_diff.clone()),
        OutputFormat::Sarif => render_sarif(&report),
        OutputFormat::Markdown => Ok(render_markdown(&report)),
        OutputFormat::Prometheus => Ok(render_prometheus(&report)),
    };

    match output_result {
        Ok(output) => {
            println!("{}", output);

            let diff_has_regression = baseline_diff
                .as_ref()
                .map(|d| d.summary.has_regression)
                .unwrap_or(false);

            if args.fail_on_regression && diff_has_regression {
                EXIT_CRITICAL_OR_ARCHIVED
            } else {
                match report.overall_status {
                    RiskLevel::Ok => EXIT_OK,
                    RiskLevel::Warn => EXIT_WARN,
                    RiskLevel::Critical | RiskLevel::Archived => EXIT_CRITICAL_OR_ARCHIVED,
                }
            }
        }
        Err(err) => {
            eprintln!("Error: {}", err);
            EXIT_INVALID_ARGS
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
