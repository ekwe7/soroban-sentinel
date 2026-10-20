//! Execution logic for the `sentinel scan` CLI command.

use sentinel_core::adapters::SorobanRpcClient;
use sentinel_core::domain::{RiskLevel, ThresholdConfig, DEFAULT_CRITICAL_LEDGERS};
use sentinel_core::error::SentinelError;
use sentinel_core::usecases::{ScanContractUseCase, ScanRequest};

use crate::args::ScanArgs;
use crate::formatters::{render_json, render_text};

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

/// Executes the contract scan command and returns the appropriate UNIX exit code.
pub async fn run_scan(args: ScanArgs) -> i32 {
    let rpc_url = match resolve_rpc_url(&args.network, args.rpc_url.as_deref()) {
        Ok(url) => url,
        Err(err) => {
            eprintln!("Error: {}", err);
            return EXIT_INVALID_ARGS;
        }
    };

    let rpc_client = SorobanRpcClient::new(rpc_url);
    let usecase = ScanContractUseCase::new(rpc_client);

    let threshold_config = ThresholdConfig {
        warning_ledgers: args.threshold,
        critical_ledgers: DEFAULT_CRITICAL_LEDGERS,
    };

    let request = ScanRequest::new(&args.contract_id)
        .with_network(&args.network)
        .with_threshold_config(threshold_config)
        .with_persistent_symbols(args.key)
        .with_persistent_xdr_keys(args.key_xdr);

    let report = match usecase.execute(request).await {
        Ok(r) => r,
        Err(err) => {
            eprintln!("Error: {}", err);
            return match err {
                SentinelError::InvalidContractAddress(_) | SentinelError::InvalidXdrKey(_) => {
                    EXIT_INVALID_ARGS
                }
                SentinelError::RpcConnectionTimeout(_) | SentinelError::RpcResponseError { .. } => {
                    EXIT_RPC_FAILURE
                }
                _ => EXIT_INVALID_ARGS,
            };
        }
    };

    let output_result = match args.format.to_lowercase().as_str() {
        "text" => Ok(render_text(&report)),
        "json" => render_json(&report),
        other => {
            eprintln!(
                "Error: Unsupported format '{}'. Supported formats: text, json",
                other
            );
            return EXIT_INVALID_ARGS;
        }
    };

    match output_result {
        Ok(output) => {
            println!("{}", output);
            match report.overall_status {
                RiskLevel::Ok => EXIT_OK,
                RiskLevel::Warn => EXIT_WARN,
                RiskLevel::Critical | RiskLevel::Archived => EXIT_CRITICAL_OR_ARCHIVED,
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
}
