//! Execution logic for the `sentinel diff-networks` CLI command.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use comfy_table::presets::ASCII_FULL;
use comfy_table::Table;
use sentinel_core::adapters::SorobanRpcClient;
use sentinel_core::domain::ThresholdConfig;
use sentinel_core::error::SentinelError;
use sentinel_core::usecases::{ScanContractUseCase, ScanRequest, StorageEntryReport, TTLReport};

use crate::args::DiffNetworksArgs;
use crate::commands::scan::{
    resolve_rpc_url, EXIT_INVALID_ARGS, EXIT_OK, EXIT_RPC_FAILURE, EXIT_WARN,
};

fn build_scan_request(args: &DiffNetworksArgs, network: &str) -> ScanRequest {
    ScanRequest::new(&args.contract_id)
        .with_network(network)
        .with_threshold_config(ThresholdConfig::default())
        .with_persistent_symbols(args.key.clone())
        .with_persistent_xdr_keys(args.key_xdr.clone())
        .with_persistent_key_exprs(args.key_expr.clone())
}

fn entry_is_present(entry: &StorageEntryReport) -> bool {
    entry.live_until_ledger != 0
}

fn format_entry_horizon(entry: Option<&StorageEntryReport>) -> String {
    match entry.filter(|entry| entry_is_present(entry)) {
        Some(entry) => format!(
            "{} remaining (until {})",
            entry.remaining_ledgers, entry.live_until_ledger
        ),
        None => "<missing>".to_string(),
    }
}

fn format_durability(entry: Option<&StorageEntryReport>) -> String {
    match entry.filter(|entry| entry_is_present(entry)) {
        Some(entry) => entry.durability.to_string(),
        None => "<missing>".to_string(),
    }
}

fn render_diff(
    report_a: &TTLReport,
    report_b: &TTLReport,
    ttl_drift_threshold: u32,
) -> (String, bool) {
    let entries_a: BTreeMap<_, _> = report_a
        .entries
        .iter()
        .map(|entry| (entry.key_identifier.as_str(), entry))
        .collect();
    let entries_b: BTreeMap<_, _> = report_b
        .entries
        .iter()
        .map(|entry| (entry.key_identifier.as_str(), entry))
        .collect();
    let keys: BTreeSet<_> = entries_a.keys().chain(entries_b.keys()).copied().collect();

    let mut significant_difference = false;
    let mut table = Table::new();
    table.load_preset(ASCII_FULL);
    table.set_header(vec![
        "Storage Key".to_string(),
        format!("{} Durability", report_a.network),
        format!("{} TTL Horizon", report_a.network),
        format!("{} Durability", report_b.network),
        format!("{} TTL Horizon", report_b.network),
        "Difference".to_string(),
    ]);

    for key in keys {
        let entry_a = entries_a.get(key).copied();
        let entry_b = entries_b.get(key).copied();
        let present_a = entry_a.is_some_and(entry_is_present);
        let present_b = entry_b.is_some_and(entry_is_present);
        let mut differences = Vec::new();

        match (present_a, present_b) {
            (false, true) => differences.push(format!("MISSING IN {}", report_a.network)),
            (true, false) => differences.push(format!("MISSING IN {}", report_b.network)),
            (false, false) => differences.push("MISSING IN BOTH".to_string()),
            (true, true) => {
                let entry_a = entry_a.expect("presence checked");
                let entry_b = entry_b.expect("presence checked");
                if entry_a.durability != entry_b.durability {
                    differences.push("DURABILITY DIFF".to_string());
                }

                let ttl_delta = entry_a
                    .remaining_ledgers
                    .abs_diff(entry_b.remaining_ledgers);
                if ttl_delta > u64::from(ttl_drift_threshold) {
                    differences.push(format!("TTL DRIFT: {} ledgers", ttl_delta));
                }
            }
        }

        if !differences.is_empty() && (present_a || present_b) {
            significant_difference = true;
        }

        table.add_row(vec![
            key.to_string(),
            format_durability(entry_a),
            format_entry_horizon(entry_a),
            format_durability(entry_b),
            format_entry_horizon(entry_b),
            if differences.is_empty() {
                "OK".to_string()
            } else {
                differences.join("; ")
            },
        ]);
    }

    (
        format!(
            "Contract: {}\nNetwork A: {} (latest ledger {})\nNetwork B: {} (latest ledger {})\nTTL drift threshold: {} ledgers\n\n{}",
            report_a.contract_id,
            report_a.network,
            report_a.latest_ledger,
            report_b.network,
            report_b.latest_ledger,
            ttl_drift_threshold,
            table
        ),
        significant_difference,
    )
}

/// Compares storage keys and TTL horizons on two Stellar networks concurrently.
pub async fn run_diff_networks(args: DiffNetworksArgs) -> i32 {
    let rpc_url_a = match resolve_rpc_url(&args.network_a, args.rpc_url_a.as_deref()) {
        Ok(url) => url,
        Err(error) => {
            eprintln!("Error: network A: {}", error);
            return EXIT_INVALID_ARGS;
        }
    };
    let rpc_url_b = match resolve_rpc_url(&args.network_b, args.rpc_url_b.as_deref()) {
        Ok(url) => url,
        Err(error) => {
            eprintln!("Error: network B: {}", error);
            return EXIT_INVALID_ARGS;
        }
    };

    let usecase_a = ScanContractUseCase::new(SorobanRpcClient::new_with_timeout(
        rpc_url_a,
        Duration::from_secs(10),
    ));
    let usecase_b = ScanContractUseCase::new(SorobanRpcClient::new_with_timeout(
        rpc_url_b,
        Duration::from_secs(10),
    ));
    let request_a = build_scan_request(&args, &args.network_a);
    let request_b = build_scan_request(&args, &args.network_b);

    let (result_a, result_b) =
        tokio::join!(usecase_a.execute(request_a), usecase_b.execute(request_b));

    let report_a = match result_a {
        Ok(report) => report,
        Err(error) => {
            eprintln!("Error scanning network A ({}): {}", args.network_a, error);
            return classify_scan_error(&error);
        }
    };
    let report_b = match result_b {
        Ok(report) => report,
        Err(error) => {
            eprintln!("Error scanning network B ({}): {}", args.network_b, error);
            return classify_scan_error(&error);
        }
    };

    let (output, significant_difference) =
        render_diff(&report_a, &report_b, args.ttl_drift_threshold);
    println!("{}", output);

    if significant_difference {
        EXIT_WARN
    } else {
        EXIT_OK
    }
}

fn classify_scan_error(error: &SentinelError) -> i32 {
    match error {
        SentinelError::InvalidContractAddress(_)
        | SentinelError::InvalidXdrKey(_)
        | SentinelError::InvalidThresholdConfig(_)
        | SentinelError::InvalidRpcUrl(_) => EXIT_INVALID_ARGS,
        SentinelError::RpcConnectionTimeout(_)
        | SentinelError::RpcResponseError { .. }
        | SentinelError::SerializationError(_) => EXIT_RPC_FAILURE,
        _ => EXIT_INVALID_ARGS,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sentinel_core::domain::{Durability, EntryType, RiskLevel};

    fn entry(
        key: &str,
        durability: Durability,
        live_until: u32,
        remaining_ledgers: i64,
    ) -> StorageEntryReport {
        StorageEntryReport::new(
            key,
            EntryType::PersistentData,
            durability,
            10,
            live_until,
            remaining_ledgers,
            0.0,
            0.0,
            RiskLevel::Ok,
        )
    }

    fn report(network: &str, entries: Vec<StorageEntryReport>) -> TTLReport {
        TTLReport {
            contract_id: "contract".to_string(),
            network: network.to_string(),
            latest_ledger: 100,
            ledger_velocity_seconds: 5.0,
            entries,
            overall_status: RiskLevel::Ok,
            contract_code_state: None,
            upgrade_event: None,
        }
    }

    #[test]
    fn identical_horizons_are_not_significant() {
        let report_a = report(
            "testnet",
            vec![entry("persistent:KEY", Durability::Persistent, 200, 100)],
        );
        let report_b = report(
            "mainnet",
            vec![entry("persistent:KEY", Durability::Persistent, 300, 100)],
        );

        let (output, significant) = render_diff(&report_a, &report_b, 10);
        assert!(!significant);
        assert!(output.contains("persistent:KEY"));
        assert!(output.contains("OK"));
    }

    #[test]
    fn detects_missing_entries_durability_and_ttl_drift() {
        let report_a = report(
            "testnet",
            vec![
                entry("persistent:KEY", Durability::Persistent, 200, 100),
                entry("instance", Durability::Instance, 150, 50),
            ],
        );
        let report_b = report(
            "mainnet",
            vec![
                entry("persistent:KEY", Durability::Temporary, 300, 80),
                entry("persistent:OTHER", Durability::Persistent, 150, 50),
            ],
        );

        let (output, significant) = render_diff(&report_a, &report_b, 10);
        assert!(significant);
        assert!(output.contains("DURABILITY DIFF"));
        assert!(output.contains("TTL DRIFT: 20 ledgers"));
        assert!(output.contains("MISSING IN mainnet"));
        assert!(output.contains("MISSING IN testnet"));
    }

    #[test]
    fn keys_missing_on_both_networks_do_not_count_as_network_drift() {
        let report_a = report(
            "testnet",
            vec![entry("persistent:KEY", Durability::Persistent, 0, 0)],
        );
        let report_b = report(
            "mainnet",
            vec![entry("persistent:KEY", Durability::Persistent, 0, 0)],
        );

        let (output, significant) = render_diff(&report_a, &report_b, 10);
        assert!(!significant);
        assert!(output.contains("MISSING IN BOTH"));
    }
}
