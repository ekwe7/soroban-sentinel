//! Network state-archival and contract entry limits.

use serde::{Deserialize, Serialize};

/// Authoritative limits returned by the network's config-setting entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkLimits {
    pub max_entry_ttl: u32,
    pub min_persistent_ttl: u32,
    pub min_temporary_ttl: u32,
    pub max_contract_data_entry_bytes: u32,
    pub protocol_version: u32,
}
