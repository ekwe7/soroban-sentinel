//! Network state-archival and contract entry limits.

use serde::{Deserialize, Serialize};

/// Maximum contract data entry size in bytes enforced by Stellar network (64 KB).
pub const MAX_CONTRACT_DATA_ENTRY_SIZE_BYTES: usize = 64 * 1024;

/// Maximum compiled contract WASM bytecode size in bytes enforced by Stellar network (64 KB).
pub const MAX_CONTRACT_SIZE_BYTES: usize = 64 * 1024;

/// Warning threshold percentage (80%) of protocol ceilings.
pub const LIMIT_WARN_THRESHOLD_PERCENT: usize = 80;

/// Authoritative limits returned by the network's config-setting entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkLimits {
    pub max_entry_ttl: u32,
    pub min_persistent_ttl: u32,
    pub min_temporary_ttl: u32,
    pub max_contract_data_entry_bytes: u32,
    pub protocol_version: u32,
}
