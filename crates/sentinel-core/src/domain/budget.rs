//! Pure rent runway and reserve budget calculations.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::domain::{Durability, EntryType};

/// Approximate Soroban ledger closes per day (5 seconds per ledger).
pub const LEDGERS_PER_DAY: u32 = 17_280;
/// Number of stroops in one XLM.
pub const STROOPS_PER_XLM: u128 = 10_000_000;

/// Entry size and classification used as input to a rent budget.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BudgetEntry {
    pub key_identifier: String,
    pub entry_type: EntryType,
    pub durability: Durability,
    pub byte_size: usize,
}

impl BudgetEntry {
    pub fn new(
        key_identifier: impl Into<String>,
        entry_type: EntryType,
        durability: Durability,
        byte_size: usize,
    ) -> Self {
        Self {
            key_identifier: key_identifier.into(),
            entry_type,
            durability,
            byte_size,
        }
    }
}

/// Itemized rent estimate for one ledger entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BudgetLineItem {
    pub key_identifier: String,
    pub entry_type: EntryType,
    pub durability: Durability,
    pub byte_size: usize,
    pub rent_stroops: u128,
    pub rent_xlm: String,
}

/// Total reserve estimate for a target horizon.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BudgetReport {
    pub horizon_days: u32,
    pub horizon_ledgers: u32,
    pub fee_rate_stroops_per_byte_ledger: u64,
    pub entries: Vec<BudgetLineItem>,
    pub total_stroops: u128,
    pub total_xlm: String,
}

/// Errors raised while validating or calculating a budget.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum BudgetError {
    #[error("horizon days must be greater than 0")]
    InvalidHorizon,
    #[error("rent estimate overflows the supported stroop range")]
    Overflow,
}

/// Calculates fee = bytes × horizon ledgers × network rate for each entry.
pub fn calculate_budget(
    entries: &[BudgetEntry],
    horizon_days: u32,
    fee_rate_stroops_per_byte_ledger: u64,
) -> Result<BudgetReport, BudgetError> {
    if horizon_days == 0 {
        return Err(BudgetError::InvalidHorizon);
    }
    let horizon_ledgers = horizon_days
        .checked_mul(LEDGERS_PER_DAY)
        .ok_or(BudgetError::Overflow)?;
    let line_items = entries
        .iter()
        .map(|entry| {
            let rent_stroops = (entry.byte_size as u128)
                .checked_mul(horizon_ledgers as u128)
                .and_then(|amount| amount.checked_mul(fee_rate_stroops_per_byte_ledger as u128))
                .ok_or(BudgetError::Overflow)?;
            Ok(BudgetLineItem {
                key_identifier: entry.key_identifier.clone(),
                entry_type: entry.entry_type,
                durability: entry.durability,
                byte_size: entry.byte_size,
                rent_stroops,
                rent_xlm: stroops_to_xlm(rent_stroops),
            })
        })
        .collect::<Result<Vec<_>, BudgetError>>()?;
    let total_stroops = line_items.iter().try_fold(0u128, |total, item| {
        total
            .checked_add(item.rent_stroops)
            .ok_or(BudgetError::Overflow)
    })?;

    Ok(BudgetReport {
        horizon_days,
        horizon_ledgers,
        fee_rate_stroops_per_byte_ledger,
        entries: line_items,
        total_stroops,
        total_xlm: stroops_to_xlm(total_stroops),
    })
}

fn stroops_to_xlm(stroops: u128) -> String {
    format!(
        "{}.{:07}",
        stroops / STROOPS_PER_XLM,
        stroops % STROOPS_PER_XLM
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calculates_itemized_budgets_for_target_horizons() {
        let entries = [BudgetEntry::new(
            "persistent:COUNTER",
            EntryType::PersistentData,
            Durability::Persistent,
            64,
        )];
        for days in [30, 90, 180, 365] {
            let report = calculate_budget(&entries, days, 2).expect("valid budget");
            let expected = 64u128 * (days as u128 * LEDGERS_PER_DAY as u128) * 2;
            assert_eq!(report.entries[0].rent_stroops, expected);
            assert_eq!(report.total_stroops, expected);
            assert_eq!(report.total_xlm, stroops_to_xlm(expected));
        }
    }

    #[test]
    fn empty_storage_and_invalid_horizons_are_handled() {
        let report = calculate_budget(&[], 30, 1).expect("empty storage");
        assert!(report.entries.is_empty());
        assert_eq!(report.total_stroops, 0);
        assert_eq!(report.total_xlm, "0.0000000");
        assert_eq!(
            calculate_budget(&[], 0, 1).unwrap_err(),
            BudgetError::InvalidHorizon
        );
    }

    #[test]
    fn rejects_horizon_and_cost_overflow() {
        assert_eq!(
            calculate_budget(&[], u32::MAX, 1).unwrap_err(),
            BudgetError::Overflow
        );
        let huge = [BudgetEntry::new(
            "huge",
            EntryType::ContractCode,
            Durability::Code,
            usize::MAX,
        )];
        assert_eq!(
            calculate_budget(&huge, 365, u64::MAX).unwrap_err(),
            BudgetError::Overflow
        );
    }
}
