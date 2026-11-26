//! Fee projections for batched TTL renewals over a common ledger horizon.

use serde::Serialize;

use super::SECONDS_PER_DAY;

/// Fixed planning rates, in stroops, for persistent Soroban storage.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct BudgetRates {
    pub rent_rate_per_kib: u64,
    pub persistent_rent_denominator: u32,
    /// All modeled non-rent costs for each transaction, including TTL writes.
    pub transaction_fee: u64,
    pub base_reserve: u64,
}

/// One live entry, with already-paid TTL expressed as ledgers from the scan.
#[derive(Debug, Clone, Copy)]
pub struct BudgetEntry {
    pub byte_size: u32,
    pub prepaid_ledgers: u32,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct ExtensionBudget {
    pub transactions: u32,
    pub rent_stroops: u128,
    pub transaction_stroops: u128,
    pub total_stroops: u128,
}

/// Convert a wall-clock target to ledgers, rounding up to cover the full duration.
pub fn duration_ledgers(days: u32, ledger_seconds: f64) -> Option<u32> {
    let ledgers = (f64::from(days) * SECONDS_PER_DAY / ledger_seconds).ceil();
    (1.0..=f64::from(u32::MAX))
        .contains(&ledgers)
        .then_some(ledgers as u32)
}

/// Model renewal to checkpoints spaced by cadence, with the last bump clipped to the horizon.
/// Entries needing renewal at the same checkpoint share one transaction.
pub fn extension_budget(
    entries: &[BudgetEntry],
    horizon: u32,
    cadence: u32,
    rates: BudgetRates,
) -> Option<ExtensionBudget> {
    if cadence == 0 || rates.persistent_rent_denominator == 0 {
        return None;
    }

    let checkpoints = horizon.div_ceil(cadence);
    let mut transactions = 0;
    let mut rent_stroops = 0;
    for entry in entries {
        if entry.prepaid_ledgers >= horizon {
            continue;
        }

        let skipped = entry.prepaid_ledgers / cadence;
        let count = checkpoints - skipped;
        transactions = transactions.max(count);
        let rent = |ledgers: u32| {
            (u128::from(entry.byte_size)
                * u128::from(ledgers)
                * u128::from(rates.rent_rate_per_kib))
            .div_ceil(1024 * u128::from(rates.persistent_rent_denominator))
        };

        if count == 1 {
            rent_stroops += rent(horizon - entry.prepaid_ledgers);
        } else {
            let first = cadence - entry.prepaid_ledgers % cadence;
            let last = (horizon - 1) % cadence + 1;
            rent_stroops += rent(first) + u128::from(count - 2) * rent(cadence) + rent(last);
        }
    }

    let transaction_stroops = u128::from(transactions) * u128::from(rates.transaction_fee);
    Some(ExtensionBudget {
        transactions,
        rent_stroops,
        transaction_stroops,
        total_stroops: rent_stroops + transaction_stroops,
    })
}

/// Smallest non-negative whole-stroop transaction fee at which the long plan
/// costs no more than the short plan. Equal transaction counts have no threshold.
pub fn break_even_transaction_fee(short: ExtensionBudget, long: ExtensionBudget) -> Option<u128> {
    let extra_transactions = short.transactions.checked_sub(long.transactions)?;
    if extra_transactions == 0 {
        return None;
    }
    Some(
        long.rent_stroops
            .saturating_sub(short.rent_stroops)
            .div_ceil(u128::from(extra_transactions)),
    )
}
