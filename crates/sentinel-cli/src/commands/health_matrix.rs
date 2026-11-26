//! One-shot fleet scorecards using the manifest's existing scan settings.
use crate::args::HealthMatrixArgs;
use crate::config::load_manifest;
use sentinel_core::adapters::SorobanRpcClient;
use sentinel_core::domain::RiskLevel;
use sentinel_core::usecases::{ScanContractUseCase, TTLReport};

/// TTL contributes 60 points, maximum entry-size headroom 20, and persistent
/// key-count headroom 20. Budgets are explicit operator scoring targets.
pub fn health_score(report: &TTLReport, warning: u32, entry_budget: u32, byte_budget: u64) -> u8 {
    let remaining = report
        .entries
        .iter()
        .map(|entry| entry.remaining_ledgers)
        .min()
        .unwrap_or(0)
        .max(0) as f64;
    let largest = report
        .entries
        .iter()
        .map(|entry| entry.byte_size)
        .max()
        .unwrap_or(0) as f64;
    let persistent = report
        .entries
        .iter()
        .filter(|entry| entry.durability == sentinel_core::domain::Durability::Persistent)
        .count();
    let ttl = (remaining / f64::from(warning.max(1))).min(1.0);
    let storage = (1.0 - largest / byte_budget.max(1) as f64).clamp(0.0, 1.0);
    let count = (1.0 - persistent as f64 / f64::from(entry_budget.max(1))).clamp(0.0, 1.0);
    let score = (60.0 * ttl + 20.0 * storage + 20.0 * count).round() as u8;
    score.min(match report.overall_status {
        RiskLevel::Ok => 100,
        RiskLevel::Warn => 70,
        RiskLevel::Critical => 40,
        RiskLevel::Archived => 0,
    })
}

pub async fn run_health_matrix(args: HealthMatrixArgs) -> i32 {
    let rpc = std::env::var("SOROBAN_RPC_URL").ok();
    let fleet = match load_manifest(&args.manifest, rpc.as_deref()) {
        Ok(fleet) => fleet,
        Err(_) => {
            eprintln!("Error: invalid fleet manifest");
            return 10;
        }
    };
    let scan = ScanContractUseCase::new(SorobanRpcClient::new(fleet.rpc_url));
    let mut table = comfy_table::Table::new();
    table.set_header([
        "Contract",
        "Health Score",
        "Entries",
        "Closest Cliff",
        "Daily Rent Est",
        "Status",
    ]);
    let mut exit = 0;
    for contract in fleet.contracts {
        let label = contract
            .name
            .as_deref()
            .unwrap_or(&contract.request.contract_id_strkey)
            .escape_default()
            .to_string();
        let warning = contract.request.threshold_config.warning_ledgers;
        match scan.execute(contract.request).await {
            Ok(report) => {
                let score = health_score(
                    &report,
                    warning,
                    args.entry_budget,
                    args.storage_budget_bytes,
                );
                exit = exit.max(match report.overall_status {
                    RiskLevel::Ok => 0,
                    RiskLevel::Warn => 1,
                    _ => 2,
                });
                if args.min_score.is_some_and(|minimum| score >= minimum) {
                    continue;
                }
                let cliff = report
                    .entries
                    .iter()
                    .map(|entry| entry.remaining_days)
                    .reduce(f64::min)
                    .unwrap_or(0.0);
                let rent = match (args.rent_fee_per_kib, args.rent_denominator) {
                    (Some(rate), Some(denominator)) => {
                        // Planning estimate at the scan's observed ledger velocity.
                        let ledgers = (86400.0 / report.ledger_velocity_seconds).ceil() as u128;
                        let bytes: u128 = report
                            .entries
                            .iter()
                            .map(|entry| entry.byte_size as u128)
                            .sum();
                        let numerator = bytes
                            .checked_mul(u128::from(rate))
                            .and_then(|value| value.checked_mul(ledgers));
                        numerator
                            .map(|value| {
                                format!(
                                    "{} stroops",
                                    value.div_ceil(1024 * u128::from(denominator))
                                )
                            })
                            .unwrap_or_else(|| "overflow".into())
                    }
                    _ => "n/a".into(),
                };
                table.add_row([
                    label,
                    score.to_string(),
                    report.entries.len().to_string(),
                    format!("{cliff:.2} days"),
                    rent,
                    report.overall_status.to_string(),
                ]);
            }
            Err(_) => {
                exit = 11;
                table.add_row([
                    label,
                    "n/a".into(),
                    "n/a".into(),
                    "n/a".into(),
                    "n/a".into(),
                    "UNKNOWN".into(),
                ]);
            }
        }
    }
    println!("{table}");
    exit
}
