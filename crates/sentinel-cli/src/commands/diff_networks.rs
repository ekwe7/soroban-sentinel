//! Compare a contract's observed storage health across two RPC networks.

use std::collections::{BTreeMap, BTreeSet};

use sentinel_core::adapters::SorobanRpcClient;
use sentinel_core::domain::{Durability, EntryType, RiskLevel};
use sentinel_core::error::SentinelError;
use sentinel_core::usecases::{ScanContractUseCase, ScanRequest, StorageEntryReport};
use serde::Serialize;

use crate::args::DiffNetworksArgs;
use crate::commands::{resolve_rpc_url, EXIT_INVALID_ARGS, EXIT_OK, EXIT_RPC_FAILURE, EXIT_WARN};

#[derive(Debug, Serialize)]
struct NetworkDiffReport {
    contract_id: String,
    network_a: String,
    network_b: String,
    latest_ledger_a: u32,
    latest_ledger_b: u32,
    ttl_drift_threshold: u32,
    significant_changes: usize,
    entries: Vec<NetworkEntryDiff>,
}

#[derive(Debug, Serialize)]
struct NetworkEntryDiff {
    key: String,
    durability_a: Option<Durability>,
    durability_b: Option<Durability>,
    entry_type_a: Option<EntryType>,
    entry_type_b: Option<EntryType>,
    status_a: Option<RiskLevel>,
    status_b: Option<RiskLevel>,
    remaining_ledgers_a: Option<i64>,
    remaining_ledgers_b: Option<i64>,
    remaining_ledgers_delta: Option<i64>,
    change: String,
    significant: bool,
}

pub async fn run_diff_networks(args: DiffNetworksArgs) -> i32 {
    let rpc_url_a = match resolve_rpc_url(&args.network_a, args.rpc_url_a.as_deref()) {
        Ok(url) => url,
        Err(_) => {
            eprintln!("Error resolving network A; use a supported network or valid --rpc-url-a");
            return EXIT_INVALID_ARGS;
        }
    };
    let rpc_url_b = match resolve_rpc_url(&args.network_b, args.rpc_url_b.as_deref()) {
        Ok(url) => url,
        Err(_) => {
            eprintln!("Error resolving network B; use a supported network or valid --rpc-url-b");
            return EXIT_INVALID_ARGS;
        }
    };

    let request_a = build_scan_request(&args, &args.network_a);
    let request_b = build_scan_request(&args, &args.network_b);
    let scan_a = ScanContractUseCase::new(SorobanRpcClient::new(rpc_url_a));
    let scan_b = ScanContractUseCase::new(SorobanRpcClient::new(rpc_url_b));
    let (result_a, result_b) = tokio::join!(
        scan_a.execute_with_keys(request_a),
        scan_b.execute_with_keys(request_b)
    );
    let (report_a, keys_a) = match result_a {
        Ok(result) => result,
        Err(error) => {
            return print_scan_error("network A", error);
        }
    };
    let (report_b, keys_b) = match result_b {
        Ok(result) => result,
        Err(error) => {
            return print_scan_error("network B", error);
        }
    };

    let observed_a = match observed_entries(&report_a.entries, &keys_a) {
        Ok(entries) => entries,
        Err(error) => {
            eprintln!("Error comparing network scans: {error}");
            return EXIT_RPC_FAILURE;
        }
    };
    let observed_b = match observed_entries(&report_b.entries, &keys_b) {
        Ok(entries) => entries,
        Err(error) => {
            eprintln!("Error comparing network scans: {error}");
            return EXIT_RPC_FAILURE;
        }
    };
    let keys: BTreeSet<_> = observed_a
        .keys()
        .chain(observed_b.keys())
        .cloned()
        .collect();
    let mut entries = Vec::with_capacity(keys.len());
    for key in keys {
        let entry_a = observed_a.get(&key).copied();
        let entry_b = observed_b.get(&key).copied();
        let missing_a = entry_a.is_none();
        let missing_b = entry_b.is_none();
        let durability_differs =
            matches!((entry_a, entry_b), (Some(a), Some(b)) if a.durability != b.durability);
        let entry_type_differs =
            matches!((entry_a, entry_b), (Some(a), Some(b)) if a.entry_type != b.entry_type);
        let status_differs =
            matches!((entry_a, entry_b), (Some(a), Some(b)) if a.status != b.status);
        let remaining_ledgers_a = entry_a.map(|entry| entry.remaining_ledgers);
        let remaining_ledgers_b = entry_b.map(|entry| entry.remaining_ledgers);
        let remaining_ledgers_delta = remaining_ledgers_a
            .zip(remaining_ledgers_b)
            .map(|(a, b)| b - a);
        let ttl_drift = remaining_ledgers_delta
            .is_some_and(|delta| delta.unsigned_abs() > u64::from(args.ttl_drift_threshold));
        let significant = missing_a
            || missing_b
            || durability_differs
            || entry_type_differs
            || status_differs
            || ttl_drift;
        let change = describe_change(
            missing_a,
            missing_b,
            durability_differs,
            entry_type_differs,
            status_differs,
            ttl_drift,
        );
        entries.push(NetworkEntryDiff {
            key: display_key(&key, entry_a.or(entry_b)),
            durability_a: entry_a.map(|entry| entry.durability),
            durability_b: entry_b.map(|entry| entry.durability),
            entry_type_a: entry_a.map(|entry| entry.entry_type),
            entry_type_b: entry_b.map(|entry| entry.entry_type),
            status_a: entry_a.map(|entry| entry.status),
            status_b: entry_b.map(|entry| entry.status),
            remaining_ledgers_a,
            remaining_ledgers_b,
            remaining_ledgers_delta,
            change,
            significant,
        });
    }

    let significant_changes = entries.iter().filter(|entry| entry.significant).count();
    let report = NetworkDiffReport {
        contract_id: args.contract_id,
        network_a: args.network_a,
        network_b: args.network_b,
        latest_ledger_a: report_a.latest_ledger,
        latest_ledger_b: report_b.latest_ledger,
        ttl_drift_threshold: args.ttl_drift_threshold,
        significant_changes,
        entries,
    };
    if args.format == "json" {
        match serde_json::to_string_pretty(&report) {
            Ok(output) => println!("{output}"),
            Err(error) => {
                eprintln!("Error serializing network comparison: {error}");
                return EXIT_RPC_FAILURE;
            }
        }
    } else {
        render_table(&report);
    }

    if significant_changes == 0 {
        EXIT_OK
    } else {
        EXIT_WARN
    }
}

fn build_scan_request(args: &DiffNetworksArgs, network: &str) -> ScanRequest {
    ScanRequest::new(&args.contract_id)
        .with_network(network)
        .with_persistent_symbols(args.key.clone())
        .with_persistent_xdr_keys(args.key_xdr.clone())
        .with_persistent_key_exprs(args.key_expr.clone())
}

fn observed_entries<'a>(
    entries: &'a [StorageEntryReport],
    keys: &'a [String],
) -> Result<BTreeMap<String, &'a StorageEntryReport>, String> {
    if entries.len() != keys.len() {
        return Err(format!(
            "scan returned {} report entries for {} ledger keys",
            entries.len(),
            keys.len()
        ));
    }
    Ok(keys
        .iter()
        .zip(entries)
        .filter(|(_, entry)| entry.live_until_ledger != 0 && entry.status != RiskLevel::Archived)
        .map(|(key, entry)| (key.clone(), entry))
        .collect())
}

fn display_key(key_xdr: &str, entry: Option<&StorageEntryReport>) -> String {
    let label = entry
        .map(|entry| entry.key_identifier.as_str())
        .unwrap_or("unknown");
    let fingerprint_len = key_xdr.len().min(12);
    format!("{label} [{}]", &key_xdr[..fingerprint_len])
}

fn describe_change(
    missing_a: bool,
    missing_b: bool,
    durability_differs: bool,
    entry_type_differs: bool,
    status_differs: bool,
    ttl_drift: bool,
) -> String {
    let mut changes = Vec::new();
    if missing_a {
        changes.push("missing from A");
    }
    if missing_b {
        changes.push("missing from B");
    }
    if durability_differs {
        changes.push("durability differs");
    }
    if entry_type_differs {
        changes.push("entry type differs");
    }
    if status_differs {
        changes.push("risk status differs");
    }
    if ttl_drift {
        changes.push("TTL drift");
    }
    if changes.is_empty() {
        "within threshold".to_string()
    } else {
        changes.join(", ")
    }
}

fn print_scan_error(network: &str, error: SentinelError) -> i32 {
    match error {
        SentinelError::InvalidContractAddress(_)
        | SentinelError::InvalidXdrKey(_)
        | SentinelError::InvalidThresholdConfig(_)
        | SentinelError::InvalidRpcUrl(_) => {
            eprintln!("Error scanning {network}: invalid contract or RPC configuration");
            EXIT_INVALID_ARGS
        }
        SentinelError::RpcResponseError { code, .. } => {
            eprintln!("Error scanning {network}: RPC request failed (code {code})");
            EXIT_RPC_FAILURE
        }
        SentinelError::RpcConnectionTimeout(_) => {
            eprintln!("Error scanning {network}: RPC connection failed or timed out");
            EXIT_RPC_FAILURE
        }
        SentinelError::SerializationError(_) => {
            eprintln!("Error scanning {network}: RPC response could not be decoded");
            EXIT_RPC_FAILURE
        }
        error => {
            eprintln!("Error scanning {network}: {error}");
            EXIT_INVALID_ARGS
        }
    }
}

fn render_table(report: &NetworkDiffReport) {
    println!(
        "{} on {} (ledger {}) vs {} (ledger {})",
        report.contract_id,
        report.network_a,
        report.latest_ledger_a,
        report.network_b,
        report.latest_ledger_b
    );
    println!(
        "{:<44} | {:<18} | {:<18} | {:>13} | change",
        "Entry key", "A durability / TTL", "B durability / TTL", "B - A ledgers"
    );
    println!("{}", "-".repeat(116));
    for entry in &report.entries {
        let a = format_side(entry.durability_a, entry.remaining_ledgers_a);
        let b = format_side(entry.durability_b, entry.remaining_ledgers_b);
        let delta = entry
            .remaining_ledgers_delta
            .map(|delta| delta.to_string())
            .unwrap_or_else(|| "-".to_string());
        println!(
            "{:<44} | {:<18} | {:<18} | {:>13} | {}",
            entry.key, a, b, delta, entry.change
        );
    }
    println!(
        "Significant changes: {} (TTL drift threshold: {} ledgers)",
        report.significant_changes, report.ttl_drift_threshold
    );
}

fn format_side(durability: Option<Durability>, remaining_ledgers: Option<i64>) -> String {
    match (durability, remaining_ledgers) {
        (Some(durability), Some(remaining)) => format!("{durability} / {remaining}"),
        _ => "missing".to_string(),
    }
}
