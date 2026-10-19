//! Abstract port definitions for querying Stellar/Soroban ledger data.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::SentinelResult;

/// Snapshot metadata of the latest ledger closed on the network.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LatestLedgerInfo {
    /// Sequence number of the latest ledger.
    pub sequence: u32,
    /// Protocol version active on the network for this ledger.
    pub protocol_version: u32,
}

impl LatestLedgerInfo {
    /// Create a new `LatestLedgerInfo`.
    pub fn new(sequence: u32, protocol_version: u32) -> Self {
        Self {
            sequence,
            protocol_version,
        }
    }
}

/// Raw representation of a ledger entry returned by a storage or RPC provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RawLedgerEntry {
    /// Base64-encoded `LedgerKey` XDR string.
    pub key_xdr_base64: String,
    /// Base64-encoded `LedgerEntryData` XDR string.
    pub entry_xdr_base64: String,
    /// Live-until ledger sequence number (expiration boundary).
    pub live_until_ledger_seq: u32,
    /// Ledger sequence number in which this entry was last modified.
    pub last_modified_ledger_seq: u32,
}

impl RawLedgerEntry {
    /// Create a new `RawLedgerEntry`.
    pub fn new(
        key_xdr_base64: impl Into<String>,
        entry_xdr_base64: impl Into<String>,
        live_until_ledger_seq: u32,
        last_modified_ledger_seq: u32,
    ) -> Self {
        Self {
            key_xdr_base64: key_xdr_base64.into(),
            entry_xdr_base64: entry_xdr_base64.into(),
            live_until_ledger_seq,
            last_modified_ledger_seq,
        }
    }
}

/// Abstract port interface for reading ledger and storage data.
///
/// Decouples domain logic and use cases from network, RPC, or storage adapters.
#[async_trait]
pub trait LedgerReader: Send + Sync {
    /// Retrieves latest ledger sequence number and active protocol version.
    async fn get_latest_ledger(&self) -> SentinelResult<LatestLedgerInfo>;

    /// Retrieves raw ledger entries for the requested base64-encoded `LedgerKey` XDRs.
    async fn get_ledger_entries(
        &self,
        keys_xdr_base64: &[String],
    ) -> SentinelResult<Vec<RawLedgerEntry>>;
}
