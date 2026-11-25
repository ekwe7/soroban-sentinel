//! Exact Soroban transaction footprint memory fee calculations.
//!
//! Soroban transactions pay explicit resource fees for declared read-only and
//! read-write ledger footprint bytes in addition to base storage rent. This
//! module measures raw XDR encoded byte lengths of [`LedgerKey`] and
//! [`LedgerEntry`] structures and applies integer ceiling division scaling:
//!
//! ```text
//! billed_kib_units = ceil(bytes / 1024)
//! fee_stroops      = billed_kib_units * rate_per_1kb
//! fee_xlm          = fee_stroops / 10_000_000
//! ```

use std::collections::HashMap;
use std::fmt;

use serde::{Deserialize, Serialize};
use stellar_xdr::curr::{
    ConfigSettingContractLedgerCostV0, ContractDataDurability, Hash, LedgerEntry, LedgerFootprint,
    LedgerKey, LedgerKeyContractCode, LedgerKeyContractData, Limits, ReadXdr, ScAddress, ScVal,
    WriteXdr,
};
use thiserror::Error;

use super::FootprintAccess;

/// Number of bytes in one Soroban fee billing unit (1 KiB).
pub const BYTES_PER_KIB: u64 = 1_024;

/// Number of stroops in 1 XLM (`10^7`).
pub const STROOPS_PER_XLM: u128 = 10_000_000;

/// Baseline Soroban testnet read fee per 1 KiB, in stroops (CAP-0046).
pub const TESTNET_FEE_READ_1KB_STROOPS: u64 = 1_786;

/// Baseline Soroban testnet write fee per 1 KiB, in stroops (CAP-0046).
pub const TESTNET_FEE_WRITE_1KB_STROOPS: u64 = 11_800;

/// Maximum recursion depth for decoding untrusted base64 XDR inputs.
const XDR_DECODE_MAX_DEPTH: u32 = 100;

/// Maximum byte length for decoding untrusted base64 XDR inputs (10 MiB).
const XDR_DECODE_MAX_LEN: usize = 10 * 1024 * 1024;

fn decode_limits() -> Limits {
    Limits {
        depth: XDR_DECODE_MAX_DEPTH,
        len: XDR_DECODE_MAX_LEN,
    }
}

/// Errors raised while measuring or calculating footprint fees.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum FootprintFeeError {
    /// Encoding a `LedgerKey` or `LedgerEntry` to raw XDR failed.
    #[error("Failed to encode XDR structure: {0}")]
    XdrEncode(String),

    /// Decoding a base64 XDR `LedgerKey` or `LedgerEntry` failed.
    #[error("Failed to decode XDR structure: {0}")]
    XdrDecode(String),

    /// Supplied network fee configuration contained an invalid (e.g. negative) rate.
    #[error("Invalid footprint fee rate: {0}")]
    InvalidFeeRate(String),

    /// Summing byte counts or stroop fees exceeded integer bounds.
    #[error("Footprint fee arithmetic overflow: {0}")]
    Overflow(String),
}

/// Classification of a footprint entry's ledger key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FootprintEntryKind {
    /// Contract instance entry (`LedgerKey::ContractData` with `ScVal::LedgerKeyContractInstance`).
    ContractInstance,
    /// Contract WASM bytecode entry (`LedgerKey::ContractCode`).
    ContractCode,
    /// Arbitrary contract storage key or other ledger entry key.
    StorageKey,
}

impl fmt::Display for FootprintEntryKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ContractInstance => write!(f, "contract_instance"),
            Self::ContractCode => write!(f, "contract_code"),
            Self::StorageKey => write!(f, "storage_key"),
        }
    }
}

/// Classifies a [`LedgerKey`] into contract instance, contract code, or arbitrary storage key.
pub fn classify_ledger_key(key: &LedgerKey) -> FootprintEntryKind {
    match key {
        LedgerKey::ContractData(data) if data.key == ScVal::LedgerKeyContractInstance => {
            FootprintEntryKind::ContractInstance
        }
        LedgerKey::ContractCode(_) => FootprintEntryKind::ContractCode,
        _ => FootprintEntryKind::StorageKey,
    }
}

/// Converts whole stroops into XLM (`stroops / 10_000_000`).
pub fn stroops_to_xlm(stroops: u128) -> f64 {
    stroops as f64 / STROOPS_PER_XLM as f64
}

/// Returns the exact raw XDR byte length of an encodable Stellar XDR structure.
pub fn xdr_encoded_len<T: WriteXdr>(value: &T) -> Result<u64, FootprintFeeError> {
    let bytes = value
        .to_xdr(Limits::none())
        .map_err(|error| FootprintFeeError::XdrEncode(error.to_string()))?;
    u64::try_from(bytes.len())
        .map_err(|_| FootprintFeeError::Overflow("encoded XDR length exceeds u64".to_string()))
}

/// Returns the number of 1 KiB (1024-byte) billing units using integer ceiling division.
pub const fn billed_kib_units(bytes: u64) -> u64 {
    bytes.div_ceil(BYTES_PER_KIB)
}

/// Applies Soroban integer ceiling division scaling: `ceil(bytes / 1024) * rate_per_1kb`.
pub const fn scale_bytes_fee(bytes: u64, rate_per_1kb: u64) -> u128 {
    (billed_kib_units(bytes) as u128) * (rate_per_1kb as u128)
}

/// Input describing one ledger entry in a transaction footprint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FootprintEntryInput {
    /// Human-readable label or key identifier for itemized reporting.
    pub label: String,
    /// Classification of the entry (contract instance, code hash, or storage key).
    pub kind: FootprintEntryKind,
    /// Access tier in the Soroban transaction footprint.
    pub access: FootprintAccess,
    /// Raw `LedgerKey` declared in the footprint.
    pub key: LedgerKey,
    /// Associated `LedgerEntry` payload loaded or written, if known.
    pub entry: Option<LedgerEntry>,
}

impl FootprintEntryInput {
    /// Creates an input from any [`LedgerKey`] and optional [`LedgerEntry`],
    /// automatically classifying its [`FootprintEntryKind`].
    pub fn new(
        label: impl Into<String>,
        access: FootprintAccess,
        key: LedgerKey,
        entry: Option<LedgerEntry>,
    ) -> Self {
        let kind = classify_ledger_key(&key);
        Self {
            label: label.into(),
            kind,
            access,
            key,
            entry,
        }
    }

    /// Builds a footprint input for a contract instance entry.
    pub fn contract_instance(
        contract_id: [u8; 32],
        access: FootprintAccess,
        entry: Option<LedgerEntry>,
    ) -> Self {
        let key = LedgerKey::ContractData(LedgerKeyContractData {
            contract: ScAddress::Contract(Hash(contract_id)),
            key: ScVal::LedgerKeyContractInstance,
            durability: ContractDataDurability::Persistent,
        });
        Self {
            label: "contract_instance".to_string(),
            kind: FootprintEntryKind::ContractInstance,
            access,
            key,
            entry,
        }
    }

    /// Builds a footprint input for a contract WASM code entry identified by its 32-byte code hash.
    pub fn contract_code(
        wasm_hash: [u8; 32],
        access: FootprintAccess,
        entry: Option<LedgerEntry>,
    ) -> Self {
        let key = LedgerKey::ContractCode(LedgerKeyContractCode {
            hash: Hash(wasm_hash),
        });
        Self {
            label: "contract_code".to_string(),
            kind: FootprintEntryKind::ContractCode,
            access,
            key,
            entry,
        }
    }

    /// Builds a footprint input for an arbitrary contract storage key (`ScVal`).
    pub fn storage_key(
        label: impl Into<String>,
        contract_id: [u8; 32],
        storage_key: ScVal,
        durability: ContractDataDurability,
        access: FootprintAccess,
        entry: Option<LedgerEntry>,
    ) -> Self {
        let key = LedgerKey::ContractData(LedgerKeyContractData {
            contract: ScAddress::Contract(Hash(contract_id)),
            key: storage_key,
            durability,
        });
        Self::new(label, access, key, entry)
    }

    /// Decodes base64 XDR `LedgerKey` and optional `LedgerEntry` into a [`FootprintEntryInput`].
    pub fn from_xdr_base64(
        label: impl Into<String>,
        access: FootprintAccess,
        key_xdr_base64: &str,
        entry_xdr_base64: Option<&str>,
    ) -> Result<Self, FootprintFeeError> {
        let limits = decode_limits();
        let key = LedgerKey::from_xdr_base64(key_xdr_base64, limits.clone())
            .map_err(|error| FootprintFeeError::XdrDecode(format!("LedgerKey: {error}")))?;
        let entry =
            match entry_xdr_base64 {
                Some(raw) => Some(LedgerEntry::from_xdr_base64(raw, limits).map_err(|error| {
                    FootprintFeeError::XdrDecode(format!("LedgerEntry: {error}"))
                })?),
                None => None,
            };
        Ok(Self::new(label, access, key, entry))
    }
}

/// Itemized byte overhead and standalone fee estimate for a single footprint entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FootprintEntryCost {
    /// Human-readable label or key identifier.
    pub label: String,
    /// Entry classification (contract instance, code hash, or arbitrary storage key).
    pub kind: FootprintEntryKind,
    /// Footprint access tier (`read_only` or `read_write`).
    pub access: FootprintAccess,
    /// Raw XDR encoded byte length of `LedgerKey`.
    pub key_bytes: u64,
    /// Raw XDR encoded byte length of `LedgerEntry` (`0` when key-only).
    pub entry_bytes: u64,
    /// Combined raw XDR byte length (`key_bytes + entry_bytes`).
    pub total_bytes: u64,
    /// Standalone fee in stroops if billed individually: `ceil(total_bytes / 1024) * rate`.
    pub standalone_fee_stroops: u128,
    /// Standalone fee in XLM (`standalone_fee_stroops / 10_000_000`).
    pub standalone_fee_xlm: f64,
}

/// Aggregated byte measurements and fee totals for one footprint partition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FootprintPartitionCost {
    /// Access tier for this partition (`read_only` or `read_write`).
    pub access: FootprintAccess,
    /// Number of entries in this partition.
    pub entry_count: usize,
    /// Sum of raw XDR `LedgerKey` bytes across all entries in this partition.
    pub key_bytes: u64,
    /// Sum of raw XDR `LedgerEntry` bytes across all entries in this partition.
    pub entry_bytes: u64,
    /// Sum of raw XDR `LedgerKey` + `LedgerEntry` bytes in this partition.
    pub total_bytes: u64,
    /// Number of billed 1 KiB units: `ceil(total_bytes / 1024)`.
    pub billed_kib_units: u64,
    /// Network byte rate applied to this partition, in stroops per 1 KiB.
    pub rate_per_1kb_stroops: u64,
    /// Partition footprint fee in whole stroops.
    pub fee_stroops: u128,
    /// Partition footprint fee in XLM (`fee_stroops / 10_000_000`).
    pub fee_xlm: f64,
}

/// Complete itemized footprint cost breakdown distinguishing read-only and read-write costs.
///
/// Aggregate partition fees (`read_only.fee_stroops` and `read_write.fee_stroops`) sum the raw
/// XDR bytes within each partition before applying integer ceiling division by 1024. Because
/// multiple sub-1 KiB entries can share a single 1 KiB billing bucket, the sum of per-entry
/// `standalone_fee_stroops` may be greater than or equal to the aggregate partition fee.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FootprintFeeBreakdown {
    /// Read-only footprint partition breakdown (billed at `fee_read_1kb`).
    pub read_only: FootprintPartitionCost,
    /// Read-write footprint partition breakdown (billed at `fee_write_1kb`).
    pub read_write: FootprintPartitionCost,
    /// Total raw XDR bytes across both partitions.
    pub total_bytes: u64,
    /// Total footprint memory fee in whole stroops (`read_only.fee_stroops + read_write.fee_stroops`).
    pub total_fee_stroops: u128,
    /// Total footprint memory fee in XLM (`total_fee_stroops / 10_000_000`).
    pub total_fee_xlm: f64,
    /// Per-entry itemized byte overhead and standalone fee breakdown.
    pub entries: Vec<FootprintEntryCost>,
}

impl FootprintFeeBreakdown {
    /// Iterates over itemized entries belonging to the read-only footprint.
    pub fn read_only_entries(&self) -> impl Iterator<Item = &FootprintEntryCost> {
        self.entries
            .iter()
            .filter(|entry| entry.access == FootprintAccess::ReadOnly)
    }

    /// Iterates over itemized entries belonging to the read-write footprint.
    pub fn read_write_entries(&self) -> impl Iterator<Item = &FootprintEntryCost> {
        self.entries
            .iter()
            .filter(|entry| entry.access == FootprintAccess::ReadWrite)
    }

    /// Iterates over itemized entries matching the requested [`FootprintEntryKind`].
    pub fn entries_by_kind(
        &self,
        kind: FootprintEntryKind,
    ) -> impl Iterator<Item = &FootprintEntryCost> {
        self.entries.iter().filter(move |entry| entry.kind == kind)
    }
}

/// Calculation engine for Soroban ledger entry footprint memory fees.
///
/// Configured with network byte rate parameters (`fee_read_1kb` and
/// `fee_write_1kb`, denominated in stroops per 1024 bytes).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FootprintFeeCalculator {
    /// Network fee in stroops per 1 KiB (1024 bytes) of read-only footprint data.
    pub fee_read_1kb: u64,
    /// Network fee in stroops per 1 KiB (1024 bytes) of read-write footprint data.
    pub fee_write_1kb: u64,
}

impl FootprintFeeCalculator {
    /// Creates a calculator configured with explicit network byte rates in stroops per 1 KiB.
    pub const fn new(fee_read_1kb: u64, fee_write_1kb: u64) -> Self {
        Self {
            fee_read_1kb,
            fee_write_1kb,
        }
    }

    /// Returns a calculator configured with the known Soroban testnet baseline rates
    /// (`fee_read_1kb = 1786`, `fee_write_1kb = 11800` stroops per 1 KiB).
    pub const fn testnet_baseline() -> Self {
        Self {
            fee_read_1kb: TESTNET_FEE_READ_1KB_STROOPS,
            fee_write_1kb: TESTNET_FEE_WRITE_1KB_STROOPS,
        }
    }

    /// Constructs a calculator from a Stellar [`ConfigSettingContractLedgerCostV0`] entry
    /// paired with the effective network write fee per 1 KiB.
    pub fn from_ledger_cost_config(
        config: &ConfigSettingContractLedgerCostV0,
        effective_write_fee_1kb: u64,
    ) -> Result<Self, FootprintFeeError> {
        let fee_read_1kb = u64::try_from(config.fee_read1_kb).map_err(|_| {
            FootprintFeeError::InvalidFeeRate(format!(
                "feeRead1KB must be non-negative, got {}",
                config.fee_read1_kb
            ))
        })?;
        Ok(Self {
            fee_read_1kb,
            fee_write_1kb: effective_write_fee_1kb,
        })
    }

    /// Returns the per-1 KiB fee rate in stroops for the given [`FootprintAccess`] tier.
    pub const fn rate_for(&self, access: FootprintAccess) -> u64 {
        match access {
            FootprintAccess::ReadOnly => self.fee_read_1kb,
            FootprintAccess::ReadWrite => self.fee_write_1kb,
        }
    }

    /// Calculates the read-only footprint fee in stroops for a raw byte count.
    pub const fn calculate_read_only_fee(&self, bytes: u64) -> u128 {
        scale_bytes_fee(bytes, self.fee_read_1kb)
    }

    /// Calculates the read-write footprint fee in stroops for a raw byte count.
    pub const fn calculate_read_write_fee(&self, bytes: u64) -> u128 {
        scale_bytes_fee(bytes, self.fee_write_1kb)
    }

    /// Calculates the footprint fee in stroops for a raw byte count under the specified access tier.
    pub const fn fee_for_bytes(&self, bytes: u64, access: FootprintAccess) -> u128 {
        scale_bytes_fee(bytes, self.rate_for(access))
    }

    /// Measures exact raw XDR byte overhead (`LedgerKey` + optional `LedgerEntry`) and
    /// standalone fee for a single footprint entry.
    pub fn measure_entry(
        &self,
        input: &FootprintEntryInput,
    ) -> Result<FootprintEntryCost, FootprintFeeError> {
        let key_bytes = xdr_encoded_len(&input.key)?;
        let entry_bytes = match &input.entry {
            Some(entry) => xdr_encoded_len(entry)?,
            None => 0,
        };
        let total_bytes = key_bytes.checked_add(entry_bytes).ok_or_else(|| {
            FootprintFeeError::Overflow("entry byte total exceeds u64".to_string())
        })?;
        let standalone_fee_stroops = self.fee_for_bytes(total_bytes, input.access);

        Ok(FootprintEntryCost {
            label: input.label.clone(),
            kind: input.kind,
            access: input.access,
            key_bytes,
            entry_bytes,
            total_bytes,
            standalone_fee_stroops,
            standalone_fee_xlm: stroops_to_xlm(standalone_fee_stroops),
        })
    }

    /// Calculates a complete itemized footprint cost breakdown across all provided entries.
    pub fn calculate(
        &self,
        entries: &[FootprintEntryInput],
    ) -> Result<FootprintFeeBreakdown, FootprintFeeError> {
        let mut itemized = Vec::with_capacity(entries.len());

        let mut ro_count = 0usize;
        let mut ro_key_bytes = 0u64;
        let mut ro_entry_bytes = 0u64;

        let mut rw_count = 0usize;
        let mut rw_key_bytes = 0u64;
        let mut rw_entry_bytes = 0u64;

        for input in entries {
            let cost = self.measure_entry(input)?;
            match cost.access {
                FootprintAccess::ReadOnly => {
                    ro_count += 1;
                    ro_key_bytes = ro_key_bytes.checked_add(cost.key_bytes).ok_or_else(|| {
                        FootprintFeeError::Overflow("read-only key bytes overflow".to_string())
                    })?;
                    ro_entry_bytes =
                        ro_entry_bytes
                            .checked_add(cost.entry_bytes)
                            .ok_or_else(|| {
                                FootprintFeeError::Overflow(
                                    "read-only entry bytes overflow".to_string(),
                                )
                            })?;
                }
                FootprintAccess::ReadWrite => {
                    rw_count += 1;
                    rw_key_bytes = rw_key_bytes.checked_add(cost.key_bytes).ok_or_else(|| {
                        FootprintFeeError::Overflow("read-write key bytes overflow".to_string())
                    })?;
                    rw_entry_bytes =
                        rw_entry_bytes
                            .checked_add(cost.entry_bytes)
                            .ok_or_else(|| {
                                FootprintFeeError::Overflow(
                                    "read-write entry bytes overflow".to_string(),
                                )
                            })?;
                }
            }
            itemized.push(cost);
        }

        let read_only = self.build_partition(
            FootprintAccess::ReadOnly,
            ro_count,
            ro_key_bytes,
            ro_entry_bytes,
        )?;
        let read_write = self.build_partition(
            FootprintAccess::ReadWrite,
            rw_count,
            rw_key_bytes,
            rw_entry_bytes,
        )?;

        let total_bytes = read_only
            .total_bytes
            .checked_add(read_write.total_bytes)
            .ok_or_else(|| {
                FootprintFeeError::Overflow("combined footprint bytes overflow".to_string())
            })?;
        let total_fee_stroops = read_only
            .fee_stroops
            .checked_add(read_write.fee_stroops)
            .ok_or_else(|| {
                FootprintFeeError::Overflow("combined footprint fee overflow".to_string())
            })?;

        Ok(FootprintFeeBreakdown {
            read_only,
            read_write,
            total_bytes,
            total_fee_stroops,
            total_fee_xlm: stroops_to_xlm(total_fee_stroops),
            entries: itemized,
        })
    }

    /// Calculates an itemized footprint breakdown directly from a Stellar [`LedgerFootprint`]
    /// and a map of [`LedgerKey`] to [`LedgerEntry`] payloads.
    ///
    /// Keys absent from `entries_by_key` are still measured for their raw `LedgerKey` byte
    /// overhead with `entry_bytes = 0`.
    pub fn calculate_ledger_footprint(
        &self,
        footprint: &LedgerFootprint,
        entries_by_key: &HashMap<LedgerKey, LedgerEntry>,
    ) -> Result<FootprintFeeBreakdown, FootprintFeeError> {
        let mut inputs = Vec::with_capacity(footprint.read_only.len() + footprint.read_write.len());

        for (index, key) in footprint.read_only.iter().enumerate() {
            let kind = classify_ledger_key(key);
            let label = format!("read_only[{index}]:{kind}");
            inputs.push(FootprintEntryInput::new(
                label,
                FootprintAccess::ReadOnly,
                key.clone(),
                entries_by_key.get(key).cloned(),
            ));
        }

        for (index, key) in footprint.read_write.iter().enumerate() {
            let kind = classify_ledger_key(key);
            let label = format!("read_write[{index}]:{kind}");
            inputs.push(FootprintEntryInput::new(
                label,
                FootprintAccess::ReadWrite,
                key.clone(),
                entries_by_key.get(key).cloned(),
            ));
        }

        self.calculate(&inputs)
    }

    fn build_partition(
        &self,
        access: FootprintAccess,
        entry_count: usize,
        key_bytes: u64,
        entry_bytes: u64,
    ) -> Result<FootprintPartitionCost, FootprintFeeError> {
        let total_bytes = key_bytes.checked_add(entry_bytes).ok_or_else(|| {
            FootprintFeeError::Overflow("partition total bytes overflow".to_string())
        })?;
        let rate_per_1kb_stroops = self.rate_for(access);
        let kib_units = billed_kib_units(total_bytes);
        let fee_stroops = scale_bytes_fee(total_bytes, rate_per_1kb_stroops);

        Ok(FootprintPartitionCost {
            access,
            entry_count,
            key_bytes,
            entry_bytes,
            total_bytes,
            billed_kib_units: kib_units,
            rate_per_1kb_stroops,
            fee_stroops,
            fee_xlm: stroops_to_xlm(fee_stroops),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use stellar_xdr::curr::ScSymbol;

    #[test]
    fn billed_kib_units_applies_exact_integer_ceil() {
        assert_eq!(billed_kib_units(0), 0);
        assert_eq!(billed_kib_units(1), 1);
        assert_eq!(billed_kib_units(1_023), 1);
        assert_eq!(billed_kib_units(1_024), 1);
        assert_eq!(billed_kib_units(1_025), 2);
        assert_eq!(billed_kib_units(2_048), 2);
        assert_eq!(billed_kib_units(2_049), 3);
    }

    #[test]
    fn classifies_instance_code_and_arbitrary_storage_keys() {
        let contract = [1u8; 32];
        let hash = [2u8; 32];

        let instance =
            FootprintEntryInput::contract_instance(contract, FootprintAccess::ReadOnly, None);
        let code = FootprintEntryInput::contract_code(hash, FootprintAccess::ReadOnly, None);
        let symbol = FootprintEntryInput::storage_key(
            "Counter",
            contract,
            ScVal::Symbol(ScSymbol("Counter".try_into().unwrap())),
            ContractDataDurability::Persistent,
            FootprintAccess::ReadWrite,
            None,
        );

        assert_eq!(instance.kind, FootprintEntryKind::ContractInstance);
        assert_eq!(code.kind, FootprintEntryKind::ContractCode);
        assert_eq!(symbol.kind, FootprintEntryKind::StorageKey);
    }

    #[test]
    fn stroops_to_xlm_converts_with_seven_decimal_places() {
        assert_eq!(stroops_to_xlm(0), 0.0);
        assert_eq!(stroops_to_xlm(10_000_000), 1.0);
        assert_eq!(stroops_to_xlm(1_786), 0.0001786);
        assert_eq!(stroops_to_xlm(11_800), 0.00118);
    }
}
