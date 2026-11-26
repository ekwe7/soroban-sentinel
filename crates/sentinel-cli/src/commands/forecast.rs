use sentinel_core::error::SentinelError;
use sentinel_core::{ForecastExtensionUseCase, ForecastRequest, SorobanRpcClient};
use serde::Serialize;
use stellar_strkey::ed25519::PublicKey as AccountStrkey;

use crate::args::ForecastArgs;
use crate::commands::scan::{resolve_rpc_url, EXIT_INVALID_ARGS, EXIT_OK, EXIT_RPC_FAILURE};

#[derive(Serialize)]
struct ForecastOutput {
    contract_id: String,
    extend_to: u32,
    latest_ledger: u32,
    min_resource_fee_stroops: u64,
    min_resource_fee_xlm: String,
    covered_entries: Vec<String>,
}

pub async fn run_forecast(args: ForecastArgs) -> i32 {
    let source = match AccountStrkey::from_string(&args.source) {
        Ok(source) => source.0,
        Err(error) => {
            eprintln!("Error: invalid source account: {error}");
            return EXIT_INVALID_ARGS;
        }
    };
    let rpc_url = match resolve_rpc_url(&args.network, args.rpc_url.as_deref()) {
        Ok(url) => url,
        Err(error) => {
            eprintln!("Error: {error}");
            return EXIT_INVALID_ARGS;
        }
    };
    let request = ForecastRequest {
        contract_id: args.contract_id,
        extend_to: args.extend_to,
        source_account: source,
        persistent_symbols: args.key,
        persistent_xdr_keys: args.key_xdr,
        persistent_key_exprs: args.key_expr,
    };
    let report = match ForecastExtensionUseCase::new(SorobanRpcClient::new(rpc_url))
        .execute(request)
        .await
    {
        Ok(report) => report,
        Err(error) => {
            eprintln!("Error: {error}");
            return match error {
                SentinelError::InvalidContractAddress(_)
                | SentinelError::InvalidXdrKey(_)
                | SentinelError::InvalidRpcUrl(_) => EXIT_INVALID_ARGS,
                _ => EXIT_RPC_FAILURE,
            };
        }
    };
    let output = ForecastOutput {
        contract_id: report.contract_id,
        extend_to: report.extend_to,
        latest_ledger: report.latest_ledger,
        min_resource_fee_xlm: xlm(report.min_resource_fee_stroops),
        min_resource_fee_stroops: report.min_resource_fee_stroops,
        covered_entries: report.covered_entries,
    };
    if args.format == "json" {
        match serde_json::to_string_pretty(&output) {
            Ok(json) => println!("{json}"),
            Err(error) => {
                eprintln!("Error: could not render forecast: {error}");
                return EXIT_RPC_FAILURE;
            }
        }
    } else {
        println!("Contract: {}", output.contract_id);
        println!("Extend to ledger: {}", output.extend_to);
        println!("Latest ledger: {}", output.latest_ledger);
        println!(
            "Minimum resource fee: {} stroops ({} XLM)",
            output.min_resource_fee_stroops, output.min_resource_fee_xlm
        );
        println!("Covered entries:");
        for entry in output.covered_entries {
            println!("  - {entry}");
        }
    }
    EXIT_OK
}

fn xlm(stroops: u64) -> String {
    format!("{}.{:07}", stroops / 10_000_000, stroops % 10_000_000)
}
