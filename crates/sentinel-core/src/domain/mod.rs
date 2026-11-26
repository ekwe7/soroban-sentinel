//! Core pure domain models and calculations.

pub mod acceleration;
pub mod alias;
pub mod budget;
pub mod bundle;
pub mod burn_rate;
pub(crate) mod cache;
pub mod cold_storage;
pub mod compare;
pub mod debt;
pub mod durability_migration;
pub mod entry;
pub mod envelope_packer;
pub mod fee_bump;
pub mod fee_optimizer;
pub mod footprint;
pub mod footprint_fees;
pub mod histogram;
pub mod hysteresis;
pub mod init_footprint;
pub mod key_pattern;
pub mod multisig;
pub mod network_limits;
pub mod overhead;
pub mod planner;
pub mod probabilistic_ttl;
pub mod recommend;
pub mod restore;
pub mod restore_fee;
pub mod risk;
pub(crate) mod scenario;
pub mod shadow_keys;
pub mod storage_audit;
pub mod ttl;
pub mod upgrade;
pub mod wasm_analysis;
pub mod wasm_graph;
pub mod wasm_imports;

pub use acceleration::{
    AcceleratedDecayWarning, AccelerationError, AccelerationReport, DecayClassification,
    EntryAcceleration, StateGrowth, StorageAccelerationTracker,
    DEFAULT_ACCELERATION_TOLERANCE_RATIO,
};
pub use alias::{AliasError, AliasRegistry};
pub use bundle::{
    plan_bundle_footprints, BundleError, BundleFootprint, MAX_BUNDLE_FOOTPRINT_ENTRIES,
};
pub use burn_rate::{
    calculate_burn_rate, project_exhaustion, validate_sample_window, BurnRate, BurnRateError,
    BurnRateSource, LedgerCloseSample, ProjectedExhaustion, DEFAULT_SAMPLE_WINDOW_LEDGERS,
    MAX_SAMPLE_WINDOW_LEDGERS, MIN_SAMPLE_WINDOW_LEDGERS,
};
pub use cache::{footprint_cache_key, SimulationCache, SimulationCacheEntry, SimulationCacheKey};
pub use compare::{
    compare_reports, BaselineDiff, BaselineDiffSummary, BaselineEntrySnapshot, BaselineSnapshot,
    ChangeCategory, EntryDiff, MAX_BASELINE_FILE_BYTES,
};
pub use debt::{RentDebt, RentDebtCalculator};
pub use entry::{Durability, EntryType, InstanceStorageBreakdown, SacAssetInfo, StorageEntry};
pub use envelope_packer::{
    calculate_batch_envelope_size, estimate_entry_footprint_bytes, pack_extension_batches,
    BASE_ENVELOPE_OVERHEAD_BYTES, DEFAULT_ENTRY_FOOTPRINT_BYTES,
};
pub use fee_bump::{
    recommend_inclusion_fee, FeeBumpError, FeePercentiles, FeeRecommendation, FeeUrgency,
};
pub use fee_optimizer::{optimize_bump_batch, FeeOptimizerEntry, ScheduledExtension, UrgencyTier};
pub use footprint::{AtRiskFootprint, FootprintAccess, FootprintValidation, FootprintValidator};
pub use footprint_fees::{
    billed_kib_units, classify_ledger_key, scale_bytes_fee, stroops_to_xlm, xdr_encoded_len,
    FootprintEntryCost, FootprintEntryInput, FootprintEntryKind, FootprintFeeBreakdown,
    FootprintFeeCalculator, FootprintFeeError, FootprintPartitionCost, BYTES_PER_KIB,
    STROOPS_PER_XLM, TESTNET_FEE_READ_1KB_STROOPS, TESTNET_FEE_WRITE_1KB_STROOPS,
};
pub use hysteresis::{AlertAction, HysteresisConfig, HysteresisFilter};
pub use key_pattern::{
    parse_key_pattern, KeyPattern, KeyPatternCandidateInput, KeyPatternToken,
    MAX_KEY_PATTERN_CANDIDATES, MAX_KEY_PATTERN_LEN,
};
pub use network_limits::NetworkLimits;
pub use overhead::{
    calculate_storage_overhead, measure_contract_data_overhead, StorageOverheadAnalysis,
    BASE_CONTRACT_DATA_WRAPPER_BYTES,
};
pub use planner::{
    plan_extension, plan_extension_with_cost, DeferralReason, DeferredExtension, ExtensionPlan,
    ExtensionPlanRequest, PlannedExtension,
};
pub use probabilistic_ttl::{
    estimate_probabilistic_lifetime, LifetimePercentiles, VelocityDistribution, VelocityModel,
};
pub use recommend::{
    build_recommendation_plan, calculate_extend_to, shell_quote, RecommendationAction,
    RecommendationPlan, TTLRecommendation, CONSERVATIVE_DEFAULT_MAX_ENTRY_TTL,
};
pub use restore::{plan_restore_footprint, RestoreError, RestorePlan};
pub use restore_fee::{RestoreFee, RestoreFeeCalculator, RestoreFeeError};
pub use risk::{
    evaluate_risk, evaluate_risk_for_durability, RiskLevel, ThresholdConfig, ThresholdOverride,
    DEFAULT_CRITICAL_LEDGERS, DEFAULT_WARNING_LEDGERS,
};
pub use scenario::{
    evaluate_protocol_scenario, NetworkRentParameters, ProtocolScenarioReport, ScenarioEntry,
    ScenarioEntryImpact, ScenarioError,
};
pub use shadow_keys::{detect_shadowed_keys, KeyCollisionKind, KeyCollisionWarning};
pub use storage_audit::{
    analyze_storage_entry, estimate_annual_rent_savings, AuditEntry, AuditFinding,
    AuditFindingKind, StorageAuditReport, MAX_SOROBAN_ENTRY_BYTES,
};
pub use ttl::{
    calculate_remaining_ledgers, estimate_horizon_days, estimate_horizon_days_with_velocity,
    estimate_remaining_days, estimate_remaining_seconds, estimate_remaining_seconds_with_velocity,
    SECONDS_PER_DAY, TARGET_LEDGER_CLOSE_SECONDS,
};
pub use upgrade::{
    detect_contract_upgrade, format_contract_code_hash, ContractUpgradeBaseline,
    ContractUpgradeEvent,
};
pub use wasm_analysis::{analyze_wasm_sections, WasmAnalysis, WasmSectionSizes};
pub use wasm_graph::{WasmDependencyGraph, WasmGraphError, WasmGraphNode, WasmRentLiability};
pub use wasm_imports::{
    scan_wasm_imports, HostSubsystem, WasmEnvironmentProfile, WasmFunctionSignature, WasmImport,
    WasmImportKind, WasmValueType,
};

pub mod grace_period;
pub use durability_migration::{
    DurabilityMigrationAnalysis, MigrationRequest, RentAnalysisError, RentRates,
};
pub use grace_period::{calculate_grace_window, GracePeriodStatus, NetworkConfig};
pub use histogram::{ExpirationBucket, ExpirationHistogram, HistogramBucket};
pub use init_footprint::{predict_initial_footprint, InitialFootprintEstimate};
pub use multisig::{
    required_cosigners, verify_bumper_authority, AccountSignerConfig, AccountThresholds,
    MultisigError, MultisigVerificationResult, SignerEntry,
};
