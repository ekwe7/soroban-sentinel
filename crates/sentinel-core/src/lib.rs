//! `sentinel-core`
//!
//! Core domain entities, risk evaluation, and Soroban RPC client adapter.

pub mod domain;
pub mod error;

pub use domain::{
    calculate_remaining_ledgers, estimate_horizon_days, estimate_remaining_days,
    estimate_remaining_seconds, Durability, EntryType, StorageEntry, SECONDS_PER_DAY,
    TARGET_LEDGER_CLOSE_SECONDS,
};
pub use error::{SentinelError, SentinelResult};
