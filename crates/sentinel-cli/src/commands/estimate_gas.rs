use std::time::Duration;

use comfy_table::Table;
use sentinel_core::{
    derive_contract_instance_key, derive_persistent_key_from_scval, derive_persistent_symbol_key,
    derive_persistent_xdr_key, parse_contract_id, parse_key_expr, Durability, EntryType,
    LedgerReader, SorobanRpcClient,
};
use serde::Serialize;
use stellar_xdr::curr::{LedgerEntryData, LedgerKey, Limits, ReadXdr, WriteXdr};

use crate::{
    args::EstimateGasArgs,
    commands::scan::{resolve_rpc_url, EXIT_INVALID_ARGS, EXIT_RPC_FAILURE},
};

const SAFETY_BUFFER_PERCENT: u64 = 15;
const BASE_CPU_INSTRUCTIONS: u64 = 75_000;
const CPU_INSTRUCTIONS_PER_ENTRY: u64 = 50_000;
const CPU_INSTRUCTIONS_PER_READ_BYTE: u64 = 100;

#[derive(Debug, Serialize)]
struct EntryEstimate {
    key: String,
    durability: &'static str,
    entry_type: &'static str,
    key_xdr_bytes: u64,
    entry_xdr_bytes: u64,
    estimated_read_bytes: u64,
    estimated_write_bytes: u64,
    estimated_cpu_instructions: u64,
}

#[derive(Debug, Serialize)]
struct EstimateReport {
    contract_id: String,
    network: String,
    extend_by_ledgers: u32,
    safety_buffer_percent: u64,
    estimated_cpu_instructions: u64,
    recommended_cpu_instructions: u64,
    estimated_read_bytes: u64,
    recommended_read_bytes: u64,
    estimated_write_bytes: u64,
    recommended_write_bytes: u64,
    estimate_method: &'static str,
    entries: Vec<EntryEstimate>,
}

pub async fn run_estimate_gas(args: EstimateGasArgs) -> i32 {
    if args.extend_by == 0 {
        eprintln!("Error: --extend-by must be greater than 0");
        return EXIT_INVALID_ARGS;
    }
    let contract_id = match parse_contract_id(&args.contract_id) {
        Ok(contract_id) => contract_id,
        Err(error) => {
            eprintln!("Error: {error}");
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

    let mut targets = vec![(
        "instance".to_owned(),
        match derive_contract_instance_key(&contract_id) {
            Ok(key) => key,
            Err(error) => {
                eprintln!("Error: {error}");
                return EXIT_INVALID_ARGS;
            }
        },
        Durability::Instance,
        EntryType::ContractInstance,
    )];
    for symbol in &args.key {
        match derive_persistent_symbol_key(&contract_id, symbol) {
            Ok(key) => targets.push((
                format!("persistent:{symbol}"),
                key,
                Durability::Persistent,
                EntryType::PersistentData,
            )),
            Err(error) => {
                eprintln!("Error: {error}");
                return EXIT_INVALID_ARGS;
            }
        }
    }
    for encoded in &args.key_xdr {
        match derive_persistent_xdr_key(&contract_id, encoded) {
            Ok(key) => targets.push((
                format!("persistent_xdr:{encoded}"),
                key,
                Durability::Persistent,
                EntryType::PersistentData,
            )),
            Err(error) => {
                eprintln!("Error: {error}");
                return EXIT_INVALID_ARGS;
            }
        }
    }
    for expression in &args.key_expr {
        let scval = match parse_key_expr(expression) {
            Ok(value) => value,
            Err(error) => {
                eprintln!("Error: {error}");
                return EXIT_INVALID_ARGS;
            }
        };
        match derive_persistent_key_from_scval(&contract_id, scval) {
            Ok(key) => targets.push((
                format!("persistent:{expression}"),
                key,
                Durability::Persistent,
                EntryType::PersistentData,
            )),
            Err(error) => {
                eprintln!("Error: {error}");
                return EXIT_INVALID_ARGS;
            }
        }
    }
    let mut seen = std::collections::HashSet::new();
    targets.retain(|(_, key, _, _)| seen.insert(key.clone()));
    let keys: Vec<String> = targets.iter().map(|(_, key, _, _)| key.clone()).collect();
    let client = SorobanRpcClient::new_with_timeout(rpc_url, Duration::from_secs(10));
    let raw_entries = match client.get_ledger_entries(&keys).await {
        Ok(entries) => entries,
        Err(error) => {
            eprintln!("Error: {error}");
            return EXIT_RPC_FAILURE;
        }
    };

    let mut estimates = Vec::with_capacity(targets.len());
    for (identifier, key_xdr, durability, entry_type) in &targets {
        let Some(raw) = raw_entries
            .iter()
            .find(|entry| entry.key_xdr_base64 == *key_xdr)
        else {
            eprintln!("Error: RPC did not return requested ledger entry {identifier}");
            return EXIT_RPC_FAILURE;
        };
        let key = match LedgerKey::from_xdr_base64(key_xdr, Limits::none()) {
            Ok(key) => key,
            Err(error) => {
                eprintln!("Error: invalid ledger key XDR: {error}");
                return EXIT_INVALID_ARGS;
            }
        };
        let data = match LedgerEntryData::from_xdr_base64(&raw.entry_xdr_base64, Limits::none()) {
            Ok(data) => data,
            Err(error) => {
                eprintln!("Error: invalid ledger entry XDR: {error}");
                return EXIT_RPC_FAILURE;
            }
        };
        let key_bytes = match key.to_xdr(Limits::none()) {
            Ok(bytes) => bytes.len() as u64,
            Err(error) => {
                eprintln!("Error: could not serialize key XDR: {error}");
                return EXIT_INVALID_ARGS;
            }
        };
        let entry_bytes = match data.to_xdr(Limits::none()) {
            Ok(bytes) => bytes.len() as u64,
            Err(error) => {
                eprintln!("Error: could not serialize entry XDR: {error}");
                return EXIT_RPC_FAILURE;
            }
        };
        let read_bytes = match key_bytes.checked_add(entry_bytes) {
            Some(bytes) => bytes,
            None => {
                eprintln!("Error: footprint byte count overflow");
                return EXIT_INVALID_ARGS;
            }
        };
        let cpu = match CPU_INSTRUCTIONS_PER_ENTRY
            .checked_add(entry_bytes.saturating_mul(CPU_INSTRUCTIONS_PER_READ_BYTE))
        {
            Some(cpu) => cpu,
            None => {
                eprintln!("Error: CPU estimate overflow");
                return EXIT_INVALID_ARGS;
            }
        };
        estimates.push(EntryEstimate {
            key: identifier.clone(),
            durability: durability_name(*durability),
            entry_type: entry_type_name(*entry_type),
            key_xdr_bytes: key_bytes,
            entry_xdr_bytes: entry_bytes,
            estimated_read_bytes: read_bytes,
            estimated_write_bytes: entry_bytes,
            estimated_cpu_instructions: cpu,
        });
    }

    let totals = estimates
        .iter()
        .try_fold((0u64, 0u64, 0u64), |(cpu, read, write), entry| {
            Some((
                cpu.checked_add(entry.estimated_cpu_instructions)?,
                read.checked_add(entry.estimated_read_bytes)?,
                write.checked_add(entry.estimated_write_bytes)?,
            ))
        });
    let Some((cpu, read, write)) = totals else {
        eprintln!("Error: total resource estimate overflow");
        return EXIT_INVALID_ARGS;
    };
    let cpu = match cpu.checked_add(BASE_CPU_INSTRUCTIONS) {
        Some(value) => value,
        None => {
            eprintln!("Error: CPU estimate overflow");
            return EXIT_INVALID_ARGS;
        }
    };
    let report = EstimateReport {
        contract_id: args.contract_id,
        network: args.network,
        extend_by_ledgers: args.extend_by,
        safety_buffer_percent: SAFETY_BUFFER_PERCENT,
        estimated_cpu_instructions: cpu,
        recommended_cpu_instructions: with_safety_buffer(cpu),
        estimated_read_bytes: read,
        recommended_read_bytes: with_safety_buffer(read),
        estimated_write_bytes: write,
        recommended_write_bytes: with_safety_buffer(write),
        estimate_method: "planning heuristic: 75,000 base CPU + 50,000 per footprint entry + 100 per entry-XDR byte; byte counts use canonical XDR",
        entries: estimates,
    };
    if args.format == "json" {
        match serde_json::to_string_pretty(&report) {
            Ok(json) => println!("{json}"),
            Err(error) => {
                eprintln!("Error: could not serialize gas estimate: {error}");
                return EXIT_INVALID_ARGS;
            }
        }
    } else {
        print_text(&report);
    }
    0
}

fn with_safety_buffer(value: u64) -> u64 {
    u64::try_from((u128::from(value) * u128::from(100 + SAFETY_BUFFER_PERCENT)).div_ceil(100))
        .unwrap_or(u64::MAX)
}

fn durability_name(value: Durability) -> &'static str {
    match value {
        Durability::Instance => "instance",
        Durability::Persistent => "persistent",
        Durability::Temporary => "temporary",
        Durability::Code => "code",
    }
}

fn entry_type_name(value: EntryType) -> &'static str {
    match value {
        EntryType::ContractInstance => "contract_instance",
        EntryType::ContractCode => "contract_code",
        EntryType::PersistentData => "contract_data",
    }
}

fn print_text(report: &EstimateReport) {
    let mut table = Table::new();
    table.set_header([
        "Key",
        "Durability",
        "Key XDR B",
        "Entry XDR B",
        "Read B",
        "Write B",
        "CPU instructions",
    ]);
    for entry in &report.entries {
        table.add_row([
            entry.key.clone(),
            entry.durability.to_owned(),
            entry.key_xdr_bytes.to_string(),
            entry.entry_xdr_bytes.to_string(),
            entry.estimated_read_bytes.to_string(),
            entry.estimated_write_bytes.to_string(),
            entry.estimated_cpu_instructions.to_string(),
        ]);
    }
    println!("{table}");
    println!(
        "Recommended with {}% buffer: CPU {}, read {} B, write {} B",
        report.safety_buffer_percent,
        report.recommended_cpu_instructions,
        report.recommended_read_bytes,
        report.recommended_write_bytes
    );
    println!(
        "CPU is a planning heuristic; simulate the final transaction for protocol-measured limits."
    );
}
