//! Core pure domain models and calculations.

pub mod entry;
pub mod risk;
pub mod ttl;

pub use entry::{Durability, EntryType, StorageEntry};
pub use risk::{
    evaluate_risk, RiskLevel, ThresholdConfig, DEFAULT_CRITICAL_LEDGERS, DEFAULT_WARNING_LEDGERS,
};
pub use ttl::{
    calculate_remaining_ledgers, estimate_horizon_days, estimate_remaining_days,
    estimate_remaining_seconds, SECONDS_PER_DAY, TARGET_LEDGER_CLOSE_SECONDS,
};
