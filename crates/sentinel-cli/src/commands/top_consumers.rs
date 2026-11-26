//! Rank the instance, code, and explicitly resolved keys from a normal scan.

use sentinel_core::adapters::{parse_contract_id, SorobanRpcClient};
use sentinel_core::usecases::{ScanContractUseCase, ScanRequest, TTLReport};
use serde::Serialize;

use super::{
    resolve_rpc_url, resolve_threshold_config, EXIT_INVALID_ARGS, EXIT_OK, EXIT_RPC_FAILURE,
};
use crate::args::{StorageScanOptions, TopConsumersArgs};

pub(super) async fn scan_entries(
    id: &str,
    options: &StorageScanOptions,
) -> Result<TTLReport, (i32, &'static str)> {
    parse_contract_id(id).map_err(|_| (EXIT_INVALID_ARGS, "invalid contract address"))?;
    let url = resolve_rpc_url(&options.network, options.rpc_url.as_deref())
        .map_err(|_| (EXIT_INVALID_ARGS, "invalid network or RPC URL"))?;
    let thresholds =
        resolve_threshold_config(options.warning_threshold, options.critical_threshold)
            .map_err(|_| (EXIT_INVALID_ARGS, "invalid warning or critical threshold"))?;
    let request = ScanRequest::new(id)
        .with_network(&options.network)
        .with_threshold_config(thresholds)
        .with_persistent_symbols(options.key.clone())
        .with_persistent_xdr_keys(options.key_xdr.clone())
        .with_persistent_key_exprs(options.key_expr.clone());
    ScanContractUseCase::new(SorobanRpcClient::new(url))
        .execute(request)
        .await
        .map_err(|error| match error {
            sentinel_core::SentinelError::InvalidContractAddress(_)
            | sentinel_core::SentinelError::InvalidXdrKey(_)
            | sentinel_core::SentinelError::InvalidRpcUrl(_)
            | sentinel_core::SentinelError::InvalidThresholdConfig(_) => {
                (EXIT_INVALID_ARGS, "invalid contract scan arguments")
            }
            _ => (EXIT_RPC_FAILURE, "contract scan failed"),
        })
}

#[derive(Serialize)]
struct Consumer<'a> {
    key_identifier: &'a str,
    byte_size: usize,
    storage_percent: f64,
}

#[derive(Serialize)]
struct ConsumersReport<'a> {
    contract_id: &'a str,
    total_entries: usize,
    total_bytes: u128,
    entries: Vec<Consumer<'a>>,
}

pub async fn run_top_consumers(args: TopConsumersArgs) -> i32 {
    let report = match scan_entries(&args.contract_id, &args.scan).await {
        Ok(report) => report,
        Err((code, error)) => {
            eprintln!("Error: {error}");
            return code;
        }
    };
    let total_bytes: u128 = report
        .entries
        .iter()
        .map(|entry| entry.byte_size as u128)
        .sum();
    let mut entries: Vec<_> = report.entries.iter().collect();
    entries.sort_by(|left, right| {
        right
            .byte_size
            .cmp(&left.byte_size)
            .then_with(|| left.key_identifier.cmp(&right.key_identifier))
    });
    let output = ConsumersReport {
        contract_id: &report.contract_id,
        total_entries: entries.len(),
        total_bytes,
        entries: entries
            .into_iter()
            .take(args.limit)
            .map(|entry| Consumer {
                key_identifier: &entry.key_identifier,
                byte_size: entry.byte_size,
                storage_percent: if total_bytes == 0 {
                    0.0
                } else {
                    entry.byte_size as f64 / total_bytes as f64 * 100.0
                },
            })
            .collect(),
    };
    if args.format == "json" {
        match serde_json::to_string_pretty(&output) {
            Ok(json) => println!("{json}"),
            Err(_) => {
                eprintln!("Error: report serialization failed");
                return EXIT_RPC_FAILURE;
            }
        }
    } else {
        println!(
            "Contract {} — {} resolved entries, {} bytes",
            output.contract_id, output.total_entries, output.total_bytes
        );
        println!(
            "{:<32} {:>12} {:>9}  Storage share",
            "Entry", "XDR bytes", "Percent"
        );
        for entry in &output.entries {
            let bars = "#".repeat((entry.storage_percent / 5.0).round() as usize);
            println!(
                "{:<32} {:>12} {:>8.2}%  [{}]",
                entry.key_identifier.escape_default(),
                entry.byte_size,
                entry.storage_percent,
                bars
            );
        }
    }
    EXIT_OK
}
