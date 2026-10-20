//! `sentinel-core`
//!
//! Core domain entities, risk evaluation, and Soroban RPC client adapter.

pub mod adapters;
pub mod domain;
pub mod error;
pub mod ports;

pub use adapters::{
    derive_contract_code_key, derive_contract_instance_key, derive_persistent_key_from_scval,
    derive_persistent_symbol_key, derive_persistent_xdr_key, extract_wasm_hash_from_instance_entry,
    parse_contract_id,
};
pub use domain::{
    calculate_remaining_ledgers, estimate_horizon_days, estimate_remaining_days,
    estimate_remaining_seconds, evaluate_risk, Durability, EntryType, RiskLevel, StorageEntry,
    ThresholdConfig, DEFAULT_CRITICAL_LEDGERS, DEFAULT_WARNING_LEDGERS, SECONDS_PER_DAY,
    TARGET_LEDGER_CLOSE_SECONDS,
};
pub use error::{SentinelError, SentinelResult};
pub use ports::{LatestLedgerInfo, LedgerReader, MockLedgerReader, RawLedgerEntry};
