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
    derive_contract_code_key, derive_contract_data_key_from_scval, derive_contract_instance_key,
    derive_persistent_key_from_scval, derive_persistent_symbol_key, derive_persistent_xdr_key,
    derive_temporary_key_from_scval, derive_temporary_symbol_key, derive_temporary_xdr_key,
    extract_wasm_hash_from_instance_entry, footprint_to_xdr_base64, parse_contract_id,
    parse_key_expr, parse_ledger_footprint, read_only_footprint, split_footprint_keys, RetryPolicy,
    SorobanRpcClient, DEFAULT_BASE_RETRY_DELAY_MS, DEFAULT_MAX_RETRIES, MAX_KEYS_PER_REQUEST,
    MAX_KEY_EXPR_DEPTH, MAX_KEY_EXPR_LEN,
};
pub use domain::{
    calculate_remaining_ledgers, compare_reports, estimate_horizon_days,
    estimate_horizon_days_with_velocity, estimate_remaining_days, estimate_remaining_seconds,
    estimate_remaining_seconds_with_velocity, evaluate_protocol_scenario, evaluate_risk,
    plan_bundle_footprints, AtRiskFootprint, BaselineDiff, BaselineDiffSummary,
    BaselineEntrySnapshot, BaselineSnapshot, BundleError, BundleFootprint, ChangeCategory,
    Durability, EntryDiff, EntryType, FootprintAccess, FootprintValidation, FootprintValidator,
    InstanceSizeThresholds, NetworkRentParameters, ProtocolScenarioReport, RiskLevel,
    ScenarioEntry, ScenarioEntryImpact, ScenarioError, StorageEntry, ThresholdConfig,
    DEFAULT_CRITICAL_LEDGERS, DEFAULT_WARNING_LEDGERS, MAX_BASELINE_FILE_BYTES,
    MAX_BUNDLE_FOOTPRINT_ENTRIES, SECONDS_PER_DAY, TARGET_LEDGER_CLOSE_SECONDS,
};
pub use error::{SentinelError, SentinelResult};
pub use ports::{LatestLedgerInfo, LedgerReader, MockLedgerReader, RawLedgerEntry};
pub use usecases::{
    compute_overall_status, InstanceStorageReport, ProtocolScenarioUseCase, ReportSummary,
    ScanContractUseCase, ScanRequest, StorageEntryReport, TTLReport,
};
