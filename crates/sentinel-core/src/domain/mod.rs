//! Core pure domain models and calculations.

pub mod compare;
pub mod entry;
pub mod fee_bump;
pub mod risk;
pub mod ttl;
pub mod upgrade;

pub use compare::{
    compare_reports, BaselineDiff, BaselineDiffSummary, BaselineEntrySnapshot, BaselineSnapshot,
    ChangeCategory, EntryDiff, MAX_BASELINE_FILE_BYTES,
};
pub use entry::{Durability, EntryType, StorageEntry};
pub use fee_bump::{
    build_replacement_fee_bump_envelope, recommend_inclusion_fee, FeePercentiles,
    FeeRecommendation, FeeStats, FeeUrgency,
};
pub use risk::{
    evaluate_risk, RiskLevel, ThresholdConfig, DEFAULT_CRITICAL_LEDGERS, DEFAULT_WARNING_LEDGERS,
};
pub use ttl::{
    calculate_remaining_ledgers, estimate_horizon_days, estimate_horizon_days_with_velocity,
    estimate_remaining_days, estimate_remaining_seconds, estimate_remaining_seconds_with_velocity,
    SECONDS_PER_DAY, TARGET_LEDGER_CLOSE_SECONDS,
};
pub use upgrade::{
    detect_contract_upgrade, ContractCodeState, ContractUpgradeEvent, DEFAULT_MINIMUM_TTL_LEDGERS,
};
