//! Contract data entry serialization overhead analysis and estimation.

use serde::{Deserialize, Serialize};
use stellar_xdr::curr::{
    ContractDataDurability, ContractDataEntry, ExtensionPoint, LedgerEntry, LedgerEntryData,
    LedgerEntryExt, Limits, ScAddress, ScVal, WriteXdr,
};

use super::{Durability, StorageEntry};

/// Analysis of metadata serialization overhead for a ledger storage entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StorageOverheadAnalysis {
    pub key_identifier: String,
    pub payload_bytes: usize,
    pub billable_bytes: usize,
    pub overhead_bytes: usize,
    pub overhead_ratio_percent: f64,
    pub recommendation: Option<String>,
}

/// Baseline overhead in bytes of a Soroban ContractData LedgerEntry wrapper.
///
/// A contract data LedgerEntry wrapper carries:
/// - last_modified_ledger_seq: 4 bytes
/// - data discriminant (ContractData): 4 bytes
/// - contract ScAddress: 36 bytes (4 bytes enum + 32 bytes hash)
/// - durability enum: 4 bytes
/// - ext ExtensionPoint V0: 4 bytes
/// - base key discriminant: 4 bytes
///
/// Summing to 56 bytes base metadata before key payload and value payload.
pub const BASE_CONTRACT_DATA_WRAPPER_BYTES: usize = 56;

/// Calculates the metadata serialization overhead for a Soroban storage entry.
pub fn calculate_storage_overhead(entry: &StorageEntry) -> StorageOverheadAnalysis {
    let billable_bytes = entry.byte_size();
    let payload_bytes = entry
        .payload_bytes()
        .unwrap_or_else(|| billable_bytes.saturating_sub(BASE_CONTRACT_DATA_WRAPPER_BYTES));

    let overhead_bytes = billable_bytes.saturating_sub(payload_bytes);
    let overhead_ratio_percent = if billable_bytes > 0 {
        ((overhead_bytes as f64) / (billable_bytes as f64)) * 100.0
    } else {
        0.0
    };

    let recommendation = if overhead_ratio_percent > 60.0 {
        Some(
            "High metadata serialization overhead (>60%). Small payloads and tiny integer keys incur disproportionate rent; consider packing multiple fields into a single struct/vector or migrating to temporary storage if state can be rebuilt."
                .to_string(),
        )
    } else if overhead_ratio_percent > 40.0 {
        Some(
            "Moderate metadata serialization overhead (>40%). Consider compacting key representations or bundling related values."
                .to_string(),
        )
    } else {
        None
    };

    StorageOverheadAnalysis {
        key_identifier: entry.key_identifier().to_string(),
        payload_bytes,
        billable_bytes,
        overhead_bytes,
        overhead_ratio_percent,
        recommendation,
    }
}

/// Measures the exact serialized overhead of a contract data entry from raw XDR types.
pub fn measure_contract_data_overhead(
    contract_id: [u8; 32],
    key: ScVal,
    val: ScVal,
    durability: Durability,
    last_modified_ledger: u32,
) -> Result<StorageOverheadAnalysis, stellar_xdr::curr::Error> {
    let raw_payload_bytes = val.to_xdr(Limits::none())?.len();

    let xdr_durability = match durability {
        Durability::Temporary => ContractDataDurability::Temporary,
        Durability::Instance | Durability::Persistent | Durability::Code => {
            ContractDataDurability::Persistent
        }
    };

    let ledger_entry = LedgerEntry {
        last_modified_ledger_seq: last_modified_ledger,
        data: LedgerEntryData::ContractData(ContractDataEntry {
            contract: ScAddress::Contract(stellar_xdr::curr::Hash(contract_id)),
            key,
            durability: xdr_durability,
            val,
            ext: ExtensionPoint::V0,
        }),
        ext: LedgerEntryExt::V0,
    };

    let billable_bytes = ledger_entry.to_xdr(Limits::none())?.len();

    let storage_entry = StorageEntry::new(
        "contract_data",
        super::EntryType::PersistentData,
        durability,
        billable_bytes,
        0,
        last_modified_ledger,
    )
    .with_payload_bytes(raw_payload_bytes);

    Ok(calculate_storage_overhead(&storage_entry))
}
