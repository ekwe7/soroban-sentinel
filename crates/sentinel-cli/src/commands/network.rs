//! Execution logic for `sentinel network`.

use std::time::Duration;

use sentinel_core::adapters::{validate_rpc_url, SorobanRpcClient};
use sentinel_core::usecases::fetch_network_limits;

use crate::args::NetworkArgs;
use crate::commands::scan::{resolve_rpc_url, EXIT_INVALID_ARGS, EXIT_RPC_FAILURE};

/// Executes the network limits command.
pub async fn run_network(args: NetworkArgs) -> i32 {
    let rpc_url = match resolve_rpc_url(&args.network, args.rpc_url.as_deref()) {
        Ok(url) => url,
        Err(error) => {
            eprintln!("Error: {error}");
            return EXIT_INVALID_ARGS;
        }
    };
    if let Err(error) = validate_rpc_url(&rpc_url) {
        eprintln!("Error: {error}");
        return EXIT_INVALID_ARGS;
    }

    let client = SorobanRpcClient::new_with_timeout(rpc_url, Duration::from_secs(10));
    match fetch_network_limits(&client).await {
        Ok(limits) => {
            if args.format == "json" {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&limits).expect("NetworkLimits is serializable")
                );
            } else {
                println!("Network: {}", args.network);
                println!("Protocol version: {}", limits.protocol_version);
                println!(
                    "Max entry TTL: {} ledgers (~{:.1} days)",
                    limits.max_entry_ttl,
                    limits.max_entry_ttl as f64 / 17280.0
                );
                println!(
                    "Min persistent TTL: {} ledgers (~{:.1} days)",
                    limits.min_persistent_ttl,
                    limits.min_persistent_ttl as f64 / 17280.0
                );
                println!(
                    "Min temporary TTL: {} ledgers (~{:.1} days)",
                    limits.min_temporary_ttl,
                    limits.min_temporary_ttl as f64 / 17280.0
                );
                println!(
                    "Max contract data entry: {} bytes",
                    limits.max_contract_data_entry_bytes
                );
            }
            0
        }
        Err(error) => {
            eprintln!("Error: {error}");
            match error {
                sentinel_core::SentinelError::InvalidRpcUrl(_) => EXIT_INVALID_ARGS,
                sentinel_core::SentinelError::InvalidContractAddress(_)
                | sentinel_core::SentinelError::InvalidXdrKey(_) => EXIT_INVALID_ARGS,
                _ => EXIT_RPC_FAILURE,
            }
        }
    }
}
