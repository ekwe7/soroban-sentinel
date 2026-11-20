//! Core pure domain models and calculations.

pub mod budget;
pub mod bundle;
pub mod burn_rate;
pub mod cold_storage;
pub mod compare;
pub mod debt;
pub mod entry;
pub mod footprint;
pub mod planner;
pub mod risk;
pub mod scenario;
pub mod ttl;

pub use bundle::{
    plan_bundle_footprints, BundleError, BundleFootprint, MAX_BUNDLE_FOOTPRINT_ENTRIES,
};
pub use burn_rate::{
    calculate_burn_rate, project_exhaustion, validate_sample_window, BurnRate, BurnRateError,
    BurnRateSource, LedgerCloseSample, ProjectedExhaustion, DEFAULT_SAMPLE_WINDOW_LEDGERS,
    MAX_SAMPLE_WINDOW_LEDGERS, MIN_SAMPLE_WINDOW_LEDGERS,
};
pub use compare::{
    compare_reports, BaselineDiff, BaselineDiffSummary, BaselineEntrySnapshot, BaselineSnapshot,
    ChangeCategory, EntryDiff, MAX_BASELINE_FILE_BYTES,
};
pub use debt::{RentDebt, RentDebtCalculator};
pub use entry::{Durability, EntryType, InstanceStorageBreakdown, SacAssetInfo, StorageEntry};
pub use footprint::{AtRiskFootprint, FootprintAccess, FootprintValidation, FootprintValidator};
pub use planner::{
    plan_extension, plan_extension_with_cost, DeferralReason, DeferredExtension, ExtensionPlan,
    ExtensionPlanRequest, PlannedExtension,
};
pub use risk::{
    evaluate_risk, RiskLevel, ThresholdConfig, DEFAULT_CRITICAL_LEDGERS, DEFAULT_WARNING_LEDGERS,
};
pub use scenario::{
    evaluate_protocol_scenario, NetworkRentParameters, ProtocolScenarioReport, ScenarioEntry,
    ScenarioEntryImpact, ScenarioError,
};
pub use ttl::{
    calculate_remaining_ledgers, estimate_horizon_days, estimate_horizon_days_with_velocity,
    estimate_remaining_days, estimate_remaining_seconds, estimate_remaining_seconds_with_velocity,
    SECONDS_PER_DAY, TARGET_LEDGER_CLOSE_SECONDS,
};
