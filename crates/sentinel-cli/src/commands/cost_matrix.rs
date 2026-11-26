//! Cost matrix execution and output for the sentinel CLI.

use comfy_table::Table;
use sentinel_core::adapters::SorobanRpcClient;
use sentinel_core::domain::cost_matrix::{
    break_even_transaction_fee, duration_ledgers, extension_budget, BudgetEntry, BudgetRates,
};
use sentinel_core::domain::TARGET_LEDGER_CLOSE_SECONDS;
use sentinel_core::error::SentinelError;
use sentinel_core::usecases::{ScanContractUseCase, ScanRequest};
use serde::Serialize;

use crate::args::CostMatrixArgs;
use crate::commands::{resolve_rpc_url, EXIT_INVALID_ARGS, EXIT_OK, EXIT_RPC_FAILURE};

const STROOPS_PER_XLM: u128 = 10_000_000;

fn xlm(stroops: u128) -> String {
    format!(
        "{}.{:07}",
        stroops / STROOPS_PER_XLM,
        stroops % STROOPS_PER_XLM
    )
}

#[derive(Serialize)]
struct Tier {
    days: u32,
    ledgers: u32,
    transactions: u32,
    rent_stroops: u128,
    rent_xlm: String,
    transaction_stroops: u128,
    total_stroops: u128,
    total_xlm: String,
}

#[derive(Serialize)]
struct Comparison {
    short_days: u32,
    long_days: u32,
    short_transactions: u32,
    long_transactions: u32,
    short_total_stroops: u128,
    long_total_stroops: u128,
    break_even_fee_stroops: Option<u128>,
    break_even_fee_xlm: Option<String>,
}

#[derive(Serialize)]
struct Matrix {
    contract_id: String,
    network: String,
    latest_ledger: u32,
    ledger_seconds: f64,
    rates: BudgetRates,
    included_entries: Vec<String>,
    skipped_entries: Vec<String>,
    minimum_account_reserve_stroops: u128,
    minimum_account_reserve_xlm: String,
    incremental_extension_reserve_stroops: u128,
    tiers: Vec<Tier>,
    comparisons: Vec<Comparison>,
    assumptions: &'static str,
}

pub async fn run_cost_matrix(mut args: CostMatrixArgs) -> i32 {
    args.tiers.sort_unstable();
    args.tiers.dedup();
    args.key.sort_unstable();
    args.key.dedup();

    let Some(ledgers) = args
        .tiers
        .iter()
        .map(|&days| duration_ledgers(days, TARGET_LEDGER_CLOSE_SECONDS))
        .collect::<Option<Vec<_>>>()
    else {
        eprintln!("Error: a duration tier exceeds the supported ledger range");
        return EXIT_INVALID_ARGS;
    };
    let rpc_url = match resolve_rpc_url(&args.network, args.rpc_url.as_deref()) {
        Ok(url) => url,
        Err(error) => {
            eprintln!("Error: {error}");
            return EXIT_INVALID_ARGS;
        }
    };
    let request = ScanRequest::new(args.contract_id)
        .with_network(args.network)
        .with_persistent_symbols(args.key)
        .with_ledger_seconds(Some(TARGET_LEDGER_CLOSE_SECONDS));
    let report = match ScanContractUseCase::new(SorobanRpcClient::new(rpc_url))
        .execute(request)
        .await
    {
        Ok(report) => report,
        Err(error) => {
            eprintln!("Error: {error}");
            return match error {
                SentinelError::InvalidContractAddress(_) | SentinelError::InvalidXdrKey(_) => {
                    EXIT_INVALID_ARGS
                }
                _ => EXIT_RPC_FAILURE,
            };
        }
    };

    let mut entries = Vec::new();
    let mut included_entries = Vec::new();
    let mut skipped_entries = Vec::new();
    for entry in report.entries {
        if let (Ok(byte_size @ 1..=u32::MAX), Ok(prepaid_ledgers)) = (
            u32::try_from(entry.byte_size),
            u32::try_from(entry.remaining_ledgers),
        ) {
            if entry.live_until_ledger != 0 {
                entries.push(BudgetEntry {
                    byte_size,
                    prepaid_ledgers,
                });
                included_entries.push(entry.key_identifier);
                continue;
            }
        }
        skipped_entries.push(entry.key_identifier);
    }

    if entries.is_empty() {
        eprintln!("Error: no live entries with known size and TTL are available for estimation");
        return EXIT_RPC_FAILURE;
    }

    let rates = BudgetRates {
        rent_rate_per_kib: args.rent_rate_per_kib,
        persistent_rent_denominator: args.persistent_rent_denominator,
        transaction_fee: args.transaction_fee,
        base_reserve: args.base_reserve,
    };
    let reserve = 2 * u128::from(rates.base_reserve);
    let mut matrix = Matrix {
        contract_id: report.contract_id,
        network: report.network,
        latest_ledger: report.latest_ledger,
        ledger_seconds: report.ledger_velocity_seconds,
        rates,
        included_entries,
        skipped_entries,
        minimum_account_reserve_stroops: reserve,
        minimum_account_reserve_xlm: xlm(reserve),
        incremental_extension_reserve_stroops: 0,
        tiers: Vec::new(),
        comparisons: Vec::new(),
        assumptions: "Planning estimates use fixed fee inputs, scanned XDR sizes and one batch per checkpoint. Check network TTL and footprint limits before using a tier.",
    };
    for (i, (&days, &horizon)) in args.tiers.iter().zip(&ledgers).enumerate() {
        let long = extension_budget(&entries, horizon, horizon, rates).expect("validated inputs");
        matrix.tiers.push(Tier {
            days,
            ledgers: horizon,
            transactions: long.transactions,
            rent_stroops: long.rent_stroops,
            rent_xlm: xlm(long.rent_stroops),
            transaction_stroops: long.transaction_stroops,
            total_stroops: long.total_stroops,
            total_xlm: xlm(long.total_stroops),
        });
        for (&short_days, &cadence) in args.tiers[..i].iter().zip(&ledgers[..i]) {
            let short =
                extension_budget(&entries, horizon, cadence, rates).expect("validated inputs");
            let threshold = break_even_transaction_fee(short, long);
            matrix.comparisons.push(Comparison {
                short_days,
                long_days: days,
                short_transactions: short.transactions,
                long_transactions: long.transactions,
                short_total_stroops: short.total_stroops,
                long_total_stroops: long.total_stroops,
                break_even_fee_stroops: threshold,
                break_even_fee_xlm: threshold.map(xlm),
            });
        }
    }

    if args.format == "json" {
        match serde_json::to_string_pretty(&matrix) {
            Ok(json) => println!("{json}"),
            Err(error) => {
                eprintln!("Error: {error}");
                return EXIT_RPC_FAILURE;
            }
        }
    } else {
        render_text(&matrix);
    }
    EXIT_OK
}

fn render_text(matrix: &Matrix) {
    println!(
        "Cost matrix for {} on {} ({} seconds per ledger)",
        matrix.contract_id, matrix.network, matrix.ledger_seconds
    );
    println!("Included entries: {}", matrix.included_entries.join(", "));
    if !matrix.skipped_entries.is_empty() {
        println!("Skipped entries: {}", matrix.skipped_entries.join(", "));
    }
    println!(
        "Minimum source account reserve: {} stroops ({} XLM), plus any account-specific reserves. TTL extensions add no reserve.",
        matrix.minimum_account_reserve_stroops, matrix.minimum_account_reserve_xlm
    );
    println!(
        "Fee assumptions: {} stroops per KiB, rent divisor {}, {} stroops per transaction.",
        matrix.rates.rent_rate_per_kib,
        matrix.rates.persistent_rent_denominator,
        matrix.rates.transaction_fee
    );
    println!("{}", matrix.assumptions);
    let mut tiers = Table::new();
    tiers.set_header([
        "Days",
        "Ledgers",
        "Transactions",
        "Rent (stroops)",
        "Rent (XLM)",
        "Total (stroops)",
        "Total (XLM)",
    ]);
    for tier in &matrix.tiers {
        tiers.add_row([
            tier.days.to_string(),
            tier.ledgers.to_string(),
            tier.transactions.to_string(),
            tier.rent_stroops.to_string(),
            tier.rent_xlm.clone(),
            tier.total_stroops.to_string(),
            tier.total_xlm.clone(),
        ]);
    }
    println!("{tiers}");
    if !matrix.comparisons.is_empty() {
        let mut comparisons = Table::new();
        comparisons.set_header([
            "Short cadence (days)",
            "Long horizon (days)",
            "Short transactions",
            "Long transactions",
            "Short total (stroops)",
            "Long total (stroops)",
            "Break-even fee (stroops)",
        ]);
        for row in &matrix.comparisons {
            comparisons.add_row([
                row.short_days.to_string(),
                row.long_days.to_string(),
                row.short_transactions.to_string(),
                row.long_transactions.to_string(),
                row.short_total_stroops.to_string(),
                row.long_total_stroops.to_string(),
                row.break_even_fee_stroops
                    .map_or_else(|| "N/A".to_owned(), |fee| fee.to_string()),
            ]);
        }
        println!("{comparisons}");
    }
}
