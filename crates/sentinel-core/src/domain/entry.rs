//! Pure domain representations of Soroban storage entries and durabilities.

use serde::{Deserialize, Serialize};

/// Durability classification for Soroban storage entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Durability {
    Instance,
    Persistent,
    Temporary,
    Code,
}

impl std::fmt::Display for Durability {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Instance => write!(f, "Instance"),
            Self::Persistent => write!(f, "Persistent"),
            Self::Temporary => write!(f, "Temporary"),
            Self::Code => write!(f, "Code"),
        }
    }
}

/// Categorization of the ledger storage entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryType {
    ContractInstance,
    ContractCode,
    PersistentData,
    TemporaryData,
}

impl std::fmt::Display for EntryType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ContractInstance => write!(f, "ContractInstance"),
            Self::ContractCode => write!(f, "ContractCode"),
            Self::PersistentData => write!(f, "PersistentData"),
            Self::TemporaryData => write!(f, "TemporaryData"),
        }
    }
}

/// Detailed breakdown of an individual key/value entry in `ScContractInstance.storage`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstanceStorageBreakdown {
    /// Human-readable summary of the storage key (e.g. symbol name, address, or type description).
    pub key_summary: String,
    /// Serialized XDR byte size of the key `ScVal`.
    pub key_bytes: usize,
    /// Serialized XDR byte size of the value `ScVal`.
    pub val_bytes: usize,
    /// Combined total byte size (key_bytes + val_bytes).
    pub total_bytes: usize,
}

impl InstanceStorageBreakdown {
    /// Creates a new `InstanceStorageBreakdown`.
    pub fn new(key_summary: impl Into<String>, key_bytes: usize, val_bytes: usize) -> Self {
        Self {
            key_summary: key_summary.into(),
            key_bytes,
            val_bytes,
            total_bytes: key_bytes + val_bytes,
        }
    }
}

/// Detailed metadata for a Stellar Asset Contract (SAC).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SacAssetInfo {
    /// Asset code (e.g., "USDC", "native").
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub asset_code: Option<String>,
    /// Issuer account Strkey address (`G...`).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub issuer: Option<String>,
    /// Asset symbol (e.g., "XLM", "USDC").
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub symbol: Option<String>,
    /// Human-readable asset name.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub name: Option<String>,
    /// Decimals precision (standard is 7 on Stellar).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub decimals: Option<u32>,
    /// Whether the asset represents the native classic XLM token.
    pub is_native: bool,
}

impl SacAssetInfo {
    /// Creates a new `SacAssetInfo`.
    pub fn new(
        asset_code: Option<String>,
        issuer: Option<String>,
        symbol: Option<String>,
        name: Option<String>,
        decimals: Option<u32>,
        is_native: bool,
    ) -> Self {
        Self {
            asset_code,
            issuer,
            symbol,
            name,
            decimals,
            is_native,
        }
    }

    /// Convenience constructor for native XLM token SAC.
    pub fn native() -> Self {
        Self {
            asset_code: Some("native".to_string()),
            issuer: None,
            symbol: Some("XLM".to_string()),
            name: Some("Native".to_string()),
            decimals: Some(7),
            is_native: true,
        }
    }
}

/// Pure domain entity representing an inspected Soroban ledger storage entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StorageEntry {
    key_identifier: String,
    entry_type: EntryType,
    durability: Durability,
    byte_size: usize,
    live_until_ledger: u32,
    last_modified_ledger: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    payload_bytes: Option<usize>,
}

impl StorageEntry {
    /// Construct a new `StorageEntry`.
    pub fn new(
        key_identifier: impl Into<String>,
        entry_type: EntryType,
        durability: Durability,
        byte_size: usize,
        live_until_ledger: u32,
        last_modified_ledger: u32,
    ) -> Self {
        Self {
            key_identifier: key_identifier.into(),
            entry_type,
            durability,
            byte_size,
            live_until_ledger,
            last_modified_ledger,
            payload_bytes: None,
        }
    }

    /// Sets the raw payload byte size (un-enveloped ScVal value size).
    pub fn with_payload_bytes(mut self, payload_bytes: usize) -> Self {
        self.payload_bytes = Some(payload_bytes);
        self
    }

    /// Returns the raw payload byte size, if known.
    pub fn payload_bytes(&self) -> Option<usize> {
        self.payload_bytes
    }

    /// Returns the human-readable or base64 key identifier.
    pub fn key_identifier(&self) -> &str {
        &self.key_identifier
    }

    /// Returns the entry type (e.g. ContractInstance, ContractCode, PersistentData).
    pub fn entry_type(&self) -> EntryType {
        self.entry_type
    }

    /// Returns the storage durability tier.
    pub fn durability(&self) -> Durability {
        self.durability
    }

    /// Returns the byte size of the storage entry.
    pub fn byte_size(&self) -> usize {
        self.byte_size
    }

    /// Returns the live-until ledger sequence number (TTL threshold).
    pub fn live_until_ledger(&self) -> u32 {
        self.live_until_ledger
    }

    /// Returns the ledger sequence number in which this entry was last modified.
    pub fn last_modified_ledger(&self) -> u32 {
        self.last_modified_ledger
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_storage_entry_constructor_and_getters() {
        let entry = StorageEntry::new(
            "test_key",
            EntryType::ContractInstance,
            Durability::Instance,
            256,
            5_500_000,
            5_400_000,
        );

        assert_eq!(entry.key_identifier(), "test_key");
        assert_eq!(entry.entry_type(), EntryType::ContractInstance);
        assert_eq!(entry.durability(), Durability::Instance);
        assert_eq!(entry.byte_size(), 256);
        assert_eq!(entry.live_until_ledger(), 5_500_000);
        assert_eq!(entry.last_modified_ledger(), 5_400_000);
    }

    #[test]
    fn test_durability_display() {
        assert_eq!(Durability::Instance.to_string(), "Instance");
        assert_eq!(Durability::Persistent.to_string(), "Persistent");
        assert_eq!(Durability::Temporary.to_string(), "Temporary");
        assert_eq!(Durability::Code.to_string(), "Code");
    }

    #[test]
    fn test_entry_type_display() {
        assert_eq!(EntryType::ContractInstance.to_string(), "ContractInstance");
        assert_eq!(EntryType::ContractCode.to_string(), "ContractCode");
        assert_eq!(EntryType::PersistentData.to_string(), "PersistentData");
    }

    #[test]
    fn test_storage_entry_serde_roundtrip() {
        let entry = StorageEntry::new(
            "CC32...:Counter",
            EntryType::PersistentData,
            Durability::Persistent,
            64,
            6_000_000,
            5_900_000,
        );

        let json = serde_json::to_string(&entry).expect("failed to serialize StorageEntry");
        let deserialized: StorageEntry =
            serde_json::from_str(&json).expect("failed to deserialize StorageEntry");

        assert_eq!(entry, deserialized);
    }
}
