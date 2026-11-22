//! `sentinel-core`
//!
//! Core domain entities, risk evaluation, and Soroban RPC client adapter.

#![forbid(unsafe_code)]

pub mod adapters;
pub mod domain;
pub mod error;
pub mod ports;
pub mod usecases;

pub use adapters::{
    analyze_wasm, build_extend_footprint_ttl_envelope, derive_contract_code_key,
    derive_contract_instance_key, derive_persistent_key_from_scval, derive_persistent_symbol_key,
    derive_persistent_xdr_key, derive_sac_allowance_key, derive_sac_balance_key,
    envelope_to_xdr_base64, extract_instance_storage_breakdown, extract_sac_asset_info,
    extract_wasm_hash_from_instance_entry, footprint_to_xdr_base64, parse_contract_id,
    parse_key_expr, parse_ledger_footprint, parse_sc_address, read_only_footprint,
    split_footprint_keys, summarize_scval, KeyPatternMatcher, ReplayReader, RetryPolicy,
    SentinelSnapshot, SnapshotHeader, SnapshotLedgerReader, SorobanRpcClient, WebhookAlertNotifier,
    DEFAULT_BASE_RETRY_DELAY_MS, DEFAULT_MAX_RETRIES, MAX_KEYS_PER_REQUEST, MAX_KEY_EXPR_DEPTH,
    MAX_KEY_EXPR_LEN, MAX_WASM_ANALYSIS_BYTES, SNAPSHOT_FORMAT_VERSION,
};
pub use domain::{
    calculate_remaining_ledgers, compare_reports, estimate_horizon_days,
    estimate_horizon_days_with_velocity, estimate_remaining_days, estimate_remaining_seconds,
    estimate_remaining_seconds_with_velocity, evaluate_protocol_scenario, evaluate_risk,
    footprint_cache_key, plan_bundle_footprints, plan_extension, plan_extension_with_cost,
    plan_restore_footprint, AtRiskFootprint, BaselineDiff, BaselineDiffSummary,
    BaselineEntrySnapshot, BaselineSnapshot, BundleError, BundleFootprint, ChangeCategory,
    DeferralReason, DeferredExtension, Durability, EntryDiff, EntryType, ExtensionPlan,
    ExtensionPlanRequest, FootprintAccess, FootprintValidation, FootprintValidator,
    InstanceStorageBreakdown, NetworkRentParameters, PlannedExtension, ProtocolScenarioReport,
    RestoreError, RestorePlan, RiskLevel, SacAssetInfo, ScenarioEntry, ScenarioEntryImpact,
    ScenarioError, SimulationCache, SimulationCacheEntry, SimulationCacheKey, StorageEntry,
    ThresholdConfig, WasmAnalysis, WasmSectionSizes, DEFAULT_CRITICAL_LEDGERS,
    DEFAULT_WARNING_LEDGERS, MAX_BASELINE_FILE_BYTES, MAX_BUNDLE_FOOTPRINT_ENTRIES,
    SECONDS_PER_DAY, TARGET_LEDGER_CLOSE_SECONDS,
};
pub use error::{SentinelError, SentinelResult};
pub use ports::{
    AlertKind, AlertMinLevel, AlertNotifier, CacheStats, CachedLedgerReader, LatestLedgerInfo,
    LedgerReader, MockLedgerReader, MockSimulationEngine, RawLedgerEntry, RentEstimate,
    SimulationEngine,
};
pub use usecases::{
    compute_overall_status, ForecastExtensionUseCase, ForecastReport, ForecastRequest,
    ProtocolScenarioUseCase, ReportSummary, ScanContractUseCase, ScanRequest, StorageEntryReport,
    TTLReport,
};
