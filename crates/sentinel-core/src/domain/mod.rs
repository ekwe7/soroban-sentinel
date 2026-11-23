//! Core pure domain models and calculations.

pub mod alias;
pub mod budget;
pub mod bundle;
pub(crate) mod cache;
pub mod cold_storage;
pub mod compare;
pub mod debt;
pub mod entry;
pub mod footprint;
pub mod key_pattern;
pub mod network_limits;
pub mod planner;
pub mod recommend;
pub mod restore;
pub mod risk;
pub(crate) mod scenario;
pub mod storage_audit;
pub mod ttl;
pub mod wasm_analysis;

pub use alias::{AliasError, AliasRegistry};
pub use bundle::{
    plan_bundle_footprints, BundleError, BundleFootprint, MAX_BUNDLE_FOOTPRINT_ENTRIES,
};
pub use cache::{footprint_cache_key, SimulationCache, SimulationCacheEntry, SimulationCacheKey};
pub use compare::{
    compare_reports, BaselineDiff, BaselineDiffSummary, BaselineEntrySnapshot, BaselineSnapshot,
    ChangeCategory, EntryDiff, MAX_BASELINE_FILE_BYTES,
};
pub use debt::{RentDebt, RentDebtCalculator};
pub use entry::{Durability, EntryType, InstanceStorageBreakdown, SacAssetInfo, StorageEntry};
pub use footprint::{AtRiskFootprint, FootprintAccess, FootprintValidation, FootprintValidator};
pub use key_pattern::{
    parse_key_pattern, KeyPattern, KeyPatternCandidateInput, KeyPatternToken,
    MAX_KEY_PATTERN_CANDIDATES, MAX_KEY_PATTERN_LEN,
};
pub use network_limits::NetworkLimits;
pub use planner::{
    plan_extension, plan_extension_with_cost, DeferralReason, DeferredExtension, ExtensionPlan,
    ExtensionPlanRequest, PlannedExtension,
};
pub use recommend::{
    build_recommendation_plan, calculate_extend_to, shell_quote, RecommendationAction,
    RecommendationPlan, TTLRecommendation, CONSERVATIVE_DEFAULT_MAX_ENTRY_TTL,
};
pub use restore::{plan_restore_footprint, RestoreError, RestorePlan};
pub use risk::{
    evaluate_risk, RiskLevel, ThresholdConfig, DEFAULT_CRITICAL_LEDGERS, DEFAULT_WARNING_LEDGERS,
};
pub use scenario::{
    evaluate_protocol_scenario, NetworkRentParameters, ProtocolScenarioReport, ScenarioEntry,
    ScenarioEntryImpact, ScenarioError,
};
pub use storage_audit::{
    analyze_storage_entry, estimate_annual_rent_savings, AuditEntry, AuditFinding,
    AuditFindingKind, StorageAuditReport, MAX_SOROBAN_ENTRY_BYTES,
};
pub use ttl::{
    calculate_remaining_ledgers, estimate_horizon_days, estimate_horizon_days_with_velocity,
    estimate_remaining_days, estimate_remaining_seconds, estimate_remaining_seconds_with_velocity,
    SECONDS_PER_DAY, TARGET_LEDGER_CLOSE_SECONDS,
};
pub use wasm_analysis::{analyze_wasm_sections, WasmAnalysis, WasmSectionSizes};
