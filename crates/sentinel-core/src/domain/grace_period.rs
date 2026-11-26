//! Protocol-accurate archival status plus an optional operator recovery target.
//! Persistent entries remain restorable after archival. An operator target
//! measures response urgency and never asserts protocol destruction.

use super::{Durability, RentDebt, RentDebtCalculator, StorageEntry};
use serde::{Deserialize, Serialize};
use std::num::{NonZeroU32, NonZeroU64};

#[derive(Debug, Clone, Copy)]
pub struct NetworkConfig {
    pub min_persistent_ttl: NonZeroU32,
    pub max_entry_ttl: NonZeroU32,
    pub ledger_close_seconds: NonZeroU32,
    pub write_fee_per_kib_stroops: u64,
    pub write_fee_per_entry_stroops: u64,
    pub persistent_rent_rate_denominator: NonZeroU64,
    /// Time after the last live ledger allowed by the operator's recovery SLO.
    pub operator_recovery_window_ledgers: Option<NonZeroU32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum GracePeriodStatus {
    Active {
        ledgers_until_expiry: u32,
        seconds_until_expiry: u64,
    },
    InGracePeriod {
        /// None denotes the protocol's unbounded restoration eligibility.
        operator_ledgers_remaining: Option<u32>,
        operator_deadline_ledger: Option<u64>,
        operator_seconds_remaining: Option<u64>,
        operator_deadline_missed: bool,
        restoration_cost: RentDebt,
    },
    PermanentlyEvicted,
    /// A missing TTL or invalid network configuration prevents a safe conclusion.
    Unknown,
}

pub fn calculate_grace_window(
    entry: &StorageEntry,
    current_ledger: u32,
    network_config: &NetworkConfig,
) -> GracePeriodStatus {
    let config = network_config;
    if entry.live_until_ledger() == 0 || config.min_persistent_ttl > config.max_entry_ttl {
        return GracePeriodStatus::Unknown;
    }
    // liveUntilLedgerSeq is inclusive: the entry remains live at this ledger.
    if current_ledger <= entry.live_until_ledger() {
        let remaining = entry.live_until_ledger() - current_ledger;
        return GracePeriodStatus::Active {
            ledgers_until_expiry: remaining,
            seconds_until_expiry: u64::from(remaining)
                * u64::from(config.ledger_close_seconds.get()),
        };
    }
    if entry.durability() == Durability::Temporary {
        return GracePeriodStatus::PermanentlyEvicted;
    }
    let Ok(bytes) = u32::try_from(entry.byte_size()) else {
        return GracePeriodStatus::Unknown;
    };
    let Some(restoration_cost) = (RentDebtCalculator {
        write_fee_per_kib_stroops: config.write_fee_per_kib_stroops,
        write_fee_per_entry_stroops: config.write_fee_per_entry_stroops,
        persistent_rent_rate_denominator: config.persistent_rent_rate_denominator,
        min_persistent_ttl: config.min_persistent_ttl,
    })
    .calculate(
        entry.durability(),
        bytes,
        entry.live_until_ledger(),
        current_ledger,
    ) else {
        return GracePeriodStatus::Unknown;
    };
    let deadline = config
        .operator_recovery_window_ledgers
        .map(|window| u64::from(entry.live_until_ledger()) + u64::from(window.get()));
    let remaining =
        deadline.map(|deadline| deadline.saturating_sub(u64::from(current_ledger)) as u32);
    GracePeriodStatus::InGracePeriod {
        operator_ledgers_remaining: remaining,
        operator_deadline_ledger: deadline,
        operator_seconds_remaining: remaining
            .map(|ledgers| u64::from(ledgers) * u64::from(config.ledger_close_seconds.get())),
        operator_deadline_missed: deadline
            .is_some_and(|deadline| u64::from(current_ledger) >= deadline),
        restoration_cost,
    }
}
