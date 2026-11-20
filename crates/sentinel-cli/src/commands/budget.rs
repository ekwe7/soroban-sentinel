//! Execution logic for `sentinel budget`.

use std::collections::HashSet;

use comfy_table::{presets::ASCII_FULL, Table};
use sentinel_core::adapters::{
    derive_temporary_key_from_scval, derive_temporary_symbol_key, derive_temporary_xdr_key,
    parse_contract_id, parse_key_expr, safe_xdr_limits, SorobanRpcClient,
};
use sentinel_core::domain::{calculate_budget, BudgetEntry, BudgetReport, Durability, EntryType};
use sentinel_core::error::SentinelError;
use sentinel_core::ports::LedgerReader;
use sentinel_core::usecases::{ScanContractUseCase, ScanRequest};
use serde::Serialize;
use stellar_xdr::curr::{LedgerEntry, LedgerEntryData, ReadXdr, WriteXdr};

use crate::args::BudgetArgs;
use crate::commands::scan::{resolve_rpc_url, EXIT_INVALID_ARGS, EXIT_RPC_FAILURE};

#[derive(Debug, Serialize)]
struct BudgetOutput {
    contract_id: String,
    network: String,
    #[serde(flatten)]
    budget: BudgetReport,
}

enum BudgetCommandError {
    Invalid(String),
    Rpc(String),
}

impl From<SentinelError> for BudgetCommandError {
    fn from(error: SentinelError) -> Self {
        match error {
            SentinelError::InvalidContractAddress(_)
            | SentinelError::InvalidXdrKey(_)
            | SentinelError::InvalidRpcUrl(_)
            | SentinelError::InvalidThresholdConfig(_) => Self::Invalid(error.to_string()),
            _ => Self::Rpc(error.to_string()),
        }
    }
}

/// Executes `sentinel budget` and returns the CLI exit code.
pub async fn run_budget(args: BudgetArgs) -> i32 {
    match run_budget_inner(args).await {
        Ok((output, format)) => {
            let rendered = if format == "json" {
                serde_json::to_string_pretty(&output).map_err(|error| error.to_string())
            } else {
                Ok(render_text(&output))
            };
            match rendered {
                Ok(rendered) => {
                    println!("{rendered}");
                    0
                }
                Err(error) => {
                    eprintln!("Error: failed to encode budget report: {error}");
                    EXIT_RPC_FAILURE
                }
            }
        }
        Err(BudgetCommandError::Invalid(message)) => {
            eprintln!("Error: {message}");
            EXIT_INVALID_ARGS
        }
        Err(BudgetCommandError::Rpc(message)) => {
            eprintln!("Error: {message}");
            EXIT_RPC_FAILURE
        }
    }
}

async fn run_budget_inner(args: BudgetArgs) -> Result<(BudgetOutput, String), BudgetCommandError> {
    if args.horizon_days == 0 || args.horizon_days > 3_650 {
        return Err(BudgetCommandError::Invalid(
            "--horizon-days must be between 1 and 3650".to_string(),
        ));
    }
    if args.fee_rate_stroops_per_byte_ledger == 0 {
        return Err(BudgetCommandError::Invalid(
            "--fee-rate-stroops-per-byte-ledger must be greater than 0".to_string(),
        ));
    }
    let contract_id_bytes = parse_contract_id(&args.contract_id)?;
    let rpc_url = resolve_rpc_url(&args.network, args.rpc_url.as_deref())
        .map_err(BudgetCommandError::Invalid)?;
    let rpc_client = SorobanRpcClient::new(rpc_url.clone());

    let scan = ScanContractUseCase::new(rpc_client.clone());
    let request = ScanRequest::new(&args.contract_id)
        .with_network(&args.network)
        .with_persistent_symbols(args.persistent_keys)
        .with_persistent_xdr_keys(args.persistent_key_xdr)
        .with_persistent_key_exprs(args.persistent_key_exprs);
    let scan_report = scan
        .execute(request)
        .await
        .map_err(BudgetCommandError::from)?;

    let mut budget_entries: Vec<_> = scan_report
        .entries
        .iter()
        .map(|entry| {
            BudgetEntry::new(
                &entry.key_identifier,
                entry.entry_type,
                entry.durability,
                entry.byte_size,
            )
        })
        .collect();

    let temporary_keys = temporary_key_descriptors(&contract_id_bytes, &args)?;
    if !temporary_keys.is_empty() {
        let query_keys: Vec<_> = temporary_keys
            .iter()
            .map(|(_, key_xdr)| key_xdr.clone())
            .collect();
        let found = rpc_client
            .get_ledger_entries(&query_keys)
            .await
            .map_err(|error| BudgetCommandError::Rpc(error.to_string()))?;
        for (identifier, key_xdr) in temporary_keys {
            let byte_size = match found.iter().find(|entry| entry.key_xdr_base64 == key_xdr) {
                Some(entry) => serialized_entry_size(&entry.entry_xdr_base64)
                    .map_err(BudgetCommandError::Rpc)?,
                None => 0,
            };
            budget_entries.push(BudgetEntry::new(
                identifier,
                EntryType::TemporaryData,
                Durability::Temporary,
                byte_size,
            ));
        }
    }

    let budget = calculate_budget(
        &budget_entries,
        args.horizon_days,
        args.fee_rate_stroops_per_byte_ledger,
    )
    .map_err(|error| BudgetCommandError::Invalid(error.to_string()))?;
    Ok((
        BudgetOutput {
            contract_id: args.contract_id,
            network: args.network,
            budget,
        },
        args.format,
    ))
}

fn temporary_key_descriptors(
    contract_id: &[u8; 32],
    args: &BudgetArgs,
) -> Result<Vec<(String, String)>, BudgetCommandError> {
    let mut descriptors = Vec::new();
    for symbol in &args.temporary_keys {
        descriptors.push((
            format!("temporary:{symbol}"),
            derive_temporary_symbol_key(contract_id, symbol)?,
        ));
    }
    for xdr in &args.temporary_key_xdr {
        descriptors.push((
            format!("temporary_xdr:{xdr}"),
            derive_temporary_xdr_key(contract_id, xdr)?,
        ));
    }
    for expression in &args.temporary_key_exprs {
        descriptors.push((
            format!("temporary:{expression}"),
            derive_temporary_key_from_scval(contract_id, parse_key_expr(expression)?)?,
        ));
    }

    let mut seen = HashSet::new();
    descriptors.retain(|(_, key)| seen.insert(key.clone()));
    Ok(descriptors)
}

fn serialized_entry_size(entry_xdr_base64: &str) -> Result<usize, String> {
    if let Ok(entry) = LedgerEntry::from_xdr_base64(entry_xdr_base64, safe_xdr_limits()) {
        return entry
            .to_xdr(safe_xdr_limits())
            .map(|bytes| bytes.len())
            .map_err(|error| format!("failed to measure temporary storage entry: {error}"));
    }
    let entry = LedgerEntryData::from_xdr_base64(entry_xdr_base64, safe_xdr_limits())
        .map_err(|error| format!("invalid temporary storage entry XDR: {error}"))?;
    entry
        .to_xdr(safe_xdr_limits())
        .map(|bytes| bytes.len())
        .map_err(|error| format!("failed to measure temporary storage entry: {error}"))
}

fn render_text(output: &BudgetOutput) -> String {
    let mut table = Table::new();
    table.load_preset(ASCII_FULL);
    table.set_header(vec![
        "Entry",
        "Durability",
        "Entry Type",
        "Bytes",
        "Rent (stroops)",
        "Rent (XLM)",
    ]);
    for entry in &output.budget.entries {
        table.add_row(vec![
            entry.key_identifier.clone(),
            entry.durability.to_string(),
            entry.entry_type.to_string(),
            entry.byte_size.to_string(),
            entry.rent_stroops.to_string(),
            entry.rent_xlm.clone(),
        ]);
    }
    format!(
        "Contract: {} | Network: {} | Horizon: {} days ({} ledgers)\nRent rate: {} stroops/byte/ledger\n{}\nTotal reserve: {} stroops ({} XLM)",
        output.contract_id,
        output.network,
        output.budget.horizon_days,
        output.budget.horizon_ledgers,
        output.budget.fee_rate_stroops_per_byte_ledger,
        table,
        output.budget.total_stroops,
        output.budget.total_xlm
    )
}
