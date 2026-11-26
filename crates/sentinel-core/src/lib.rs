//! `sentinel-core`
//!
//! Core domain entities, risk evaluation, and Soroban RPC client adapter.

#![forbid(unsafe_code)]

pub mod adapters;
pub mod domain;
pub mod error;
pub mod ports;
pub mod usecases;

pub use adapters::*;
pub use domain::fee_bump;
pub use domain::*;
pub use domain::{
    break_even_transaction_fee, calculate_budget, calculate_remaining_ledgers, compare_reports,
    duration_ledgers, estimate_horizon_days, estimate_horizon_days_with_velocity,
    estimate_remaining_days, estimate_remaining_seconds, estimate_remaining_seconds_with_velocity,
    evaluate_protocol_scenario, evaluate_risk, extension_budget, plan_bundle_footprints,
    plan_extension, plan_extension_with_cost, plan_restore_footprint, AtRiskFootprint,
    BaselineDiff, BaselineDiffSummary, BaselineEntrySnapshot, BaselineSnapshot, BudgetEntry,
    BudgetError, BudgetLineItem, BudgetRates, BudgetReport, BundleError, BundleFootprint,
    ChangeCategory, DeferralReason, DeferredExtension, Durability, EntryDiff, EntryType,
    ExtensionBudget, ExtensionPlan, ExtensionPlanRequest, FootprintAccess, FootprintValidation,
    FootprintValidator, InstanceStorageBreakdown, NetworkRentParameters, PlannedExtension,
    ProtocolScenarioReport, RestoreError, RestorePlan, RiskLevel, SacAssetInfo, ScenarioEntry,
    ScenarioEntryImpact, ScenarioError, SorobanInterfaceVersion, StorageEntry, ThresholdConfig,
    WasmCodeInspection, WasmSectionSize, DEFAULT_CRITICAL_LEDGERS, DEFAULT_MAX_WASM_SIZE_BYTES,
    DEFAULT_WARNING_LEDGERS, LEDGERS_PER_DAY, MAX_BASELINE_FILE_BYTES,
    MAX_BUNDLE_FOOTPRINT_ENTRIES, SECONDS_PER_DAY, STROOPS_PER_XLM, TARGET_LEDGER_CLOSE_SECONDS,
};
pub use error::{SentinelError, SentinelResult};
pub use ports::*;
pub use ports::{
    CacheStats, CachedLedgerReader, LatestLedgerInfo, LedgerReader, MockLedgerReader,
    RawLedgerEntry,
};
pub use usecases::*;
pub use usecases::{
    compute_overall_status, ProtocolScenarioUseCase, ReportSummary, ScanContractUseCase,
    ScanRequest, StorageEntryReport, TTLReport,
};
