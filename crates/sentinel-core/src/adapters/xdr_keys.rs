//! Deterministic XDR key derivation adapter using `stellar-xdr` and `stellar-strkey`.

use std::str::FromStr;
use stellar_strkey::Contract;
use stellar_xdr::curr::{
    ConfigSettingEntry, ConfigSettingId, ContractDataDurability, ContractExecutable, Hash,
    LedgerEntry, LedgerEntryData, LedgerEntryExt, LedgerKey, LedgerKeyConfigSetting,
    LedgerKeyContractCode, LedgerKeyContractData, Limits, ReadXdr, ScAddress, ScSymbol, ScVal,
    WriteXdr,
};

use crate::error::{SentinelError, SentinelResult};

/// Parses a Stellar Strkey contract address (e.g., `CA3D...`) into a 32-byte contract ID.
pub fn parse_contract_id(strkey: &str) -> SentinelResult<[u8; 32]> {
    Contract::from_str(strkey)
        .map(|c| c.0)
        .map_err(|e| SentinelError::InvalidContractAddress(format!("{}: {}", strkey, e)))
}

/// Maximum recursion depth allowed during XDR deserialization to prevent stack exhaustion.
pub const XDR_MAX_RECURSION_DEPTH: u32 = 100;

/// Maximum payload byte length allowed during XDR deserialization (10 MB).
pub const XDR_MAX_BYTE_LENGTH: usize = 10 * 1024 * 1024;

/// Returns standard bounded limits for safe XDR deserialization.
pub fn safe_xdr_limits() -> Limits {
    Limits {
        depth: XDR_MAX_RECURSION_DEPTH,
        len: XDR_MAX_BYTE_LENGTH,
    }
}

/// Derives the base64-encoded `LedgerKey` XDR for a contract's instance data (`ContractDataDurability::Persistent`).
pub fn derive_contract_instance_key(contract_id: &[u8; 32]) -> SentinelResult<String> {
    let ledger_key = LedgerKey::ContractData(LedgerKeyContractData {
        contract: ScAddress::Contract(Hash(*contract_id)),
        key: ScVal::LedgerKeyContractInstance,
        durability: ContractDataDurability::Persistent,
    });

    ledger_key.to_xdr_base64(Limits::none()).map_err(|e| {
        SentinelError::SerializationError(format!("Failed to encode instance key: {}", e))
    })
}

/// Derives the base64-encoded `LedgerKey` XDR for a contract's WASM bytecode (`LedgerKey::ContractCode`).
pub fn derive_contract_code_key(wasm_hash: &[u8; 32]) -> SentinelResult<String> {
    let ledger_key = LedgerKey::ContractCode(LedgerKeyContractCode {
        hash: Hash(*wasm_hash),
    });

    ledger_key
        .to_xdr_base64(Limits::none())
        .map_err(|e| SentinelError::SerializationError(format!("Failed to encode code key: {}", e)))
}

/// Extracts the WASM executable hash from an instance entry's base64-encoded XDR data.
///
/// Accepts base64 XDR corresponding to either a full [`LedgerEntry`] or [`LedgerEntryData`].
/// Returns `Ok(Some([u8; 32]))` if the executable is WASM, or `Ok(None)` for built-in contracts.
pub fn extract_wasm_hash_from_instance_entry(
    entry_xdr_base64: &str,
) -> SentinelResult<Option<[u8; 32]>> {
    let entry_data =
        if let Ok(entry) = LedgerEntry::from_xdr_base64(entry_xdr_base64, safe_xdr_limits()) {
            entry.data
        } else {
            LedgerEntryData::from_xdr_base64(entry_xdr_base64, safe_xdr_limits()).map_err(|e| {
                SentinelError::InvalidXdrKey(format!(
                    "Failed to decode base64 instance entry XDR: {}",
                    e
                ))
            })?
        };

    if let LedgerEntryData::ContractData(contract_data) = entry_data {
        if let ScVal::ContractInstance(instance) = contract_data.val {
            match instance.executable {
                ContractExecutable::Wasm(hash) => Ok(Some(hash.0)),
                ContractExecutable::StellarAsset => Ok(None),
            }
        } else {
            Ok(None)
        }
    } else {
        Err(SentinelError::InvalidXdrKey(
            "Decoded ledger entry is not ContractData".to_string(),
        ))
    }
}

/// Decodes base64 XDR holding either a full [`LedgerEntry`] or bare [`LedgerEntryData`]
/// (as returned by `getLedgerEntries`), bounded by [`safe_xdr_limits`].
///
/// Bare data is wrapped in a `LedgerEntry` with a V0 extension, so its serialized size matches
/// what the network measures for the stored entry.
fn decode_ledger_entry(entry_xdr_base64: &str) -> SentinelResult<LedgerEntry> {
    if let Ok(entry) = LedgerEntry::from_xdr_base64(entry_xdr_base64, safe_xdr_limits()) {
        return Ok(entry);
    }
    let data =
        LedgerEntryData::from_xdr_base64(entry_xdr_base64, safe_xdr_limits()).map_err(|e| {
            SentinelError::InvalidXdrKey(format!("Failed to decode ledger entry XDR: {}", e))
        })?;
    Ok(LedgerEntry {
        last_modified_ledger_seq: 0,
        data,
        ext: LedgerEntryExt::V0,
    })
}

/// Key count and serialized size of a contract instance entry's storage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InstanceStorageStats {
    /// Number of keys in the instance's `ScContractInstance.storage` map (`0` when absent).
    pub key_count: u32,
    /// Serialized size of the whole instance `LedgerEntry` in bytes: the size the network
    /// compares against `max_contract_data_entry_bytes`.
    pub entry_size_bytes: usize,
}

/// Reads instance-storage key count and entry size from an instance entry's base64 XDR.
///
/// Only the map's length is read; storage values are never decoded into anything else. An absent
/// storage map counts as zero keys. Malformed XDR or a non-instance entry returns an error.
pub fn extract_instance_storage_stats(
    entry_xdr_base64: &str,
) -> SentinelResult<InstanceStorageStats> {
    let entry = decode_ledger_entry(entry_xdr_base64)?;
    let LedgerEntryData::ContractData(contract_data) = &entry.data else {
        return Err(SentinelError::InvalidXdrKey(
            "Decoded ledger entry is not ContractData".to_string(),
        ));
    };
    let ScVal::ContractInstance(instance) = &contract_data.val else {
        return Err(SentinelError::InvalidXdrKey(
            "Contract data entry is not a contract instance".to_string(),
        ));
    };
    let key_count = instance
        .storage
        .as_ref()
        .map_or(0, |storage| storage.len() as u32);
    let entry_size_bytes = entry
        .to_xdr(safe_xdr_limits())
        .map_err(|e| {
            SentinelError::SerializationError(format!("Failed to encode instance entry: {}", e))
        })?
        .len();
    Ok(InstanceStorageStats {
        key_count,
        entry_size_bytes,
    })
}

/// Derives the base64-encoded `LedgerKey` XDR for the network's maximum contract-data entry size
/// (`ConfigSettingId::ContractDataEntrySizeBytes`).
pub fn derive_contract_data_entry_size_limit_key() -> SentinelResult<String> {
    LedgerKey::ConfigSetting(LedgerKeyConfigSetting {
        config_setting_id: ConfigSettingId::ContractDataEntrySizeBytes,
    })
    .to_xdr_base64(Limits::none())
    .map_err(|e| {
        SentinelError::SerializationError(format!("Failed to encode config setting key: {}", e))
    })
}

/// Reads `max_contract_data_entry_bytes` from a `ContractDataEntrySizeBytes` config-setting
/// entry's base64 XDR.
pub fn extract_contract_data_entry_size_limit(entry_xdr_base64: &str) -> SentinelResult<u32> {
    match decode_ledger_entry(entry_xdr_base64)?.data {
        LedgerEntryData::ConfigSetting(ConfigSettingEntry::ContractDataEntrySizeBytes(limit)) => {
            Ok(limit)
        }
        _ => Err(SentinelError::InvalidXdrKey(
            "Ledger entry is not the ContractDataEntrySizeBytes config setting".to_string(),
        )),
    }
}

/// Derives the base64-encoded `LedgerKey` XDR for a contract-data key with the given durability.
///
/// Persistent and temporary keys share this encoding; only `durability` differs, so the same
/// `ScVal` yields a different `LedgerKey` in each storage tier.
pub fn derive_contract_data_key_from_scval(
    contract_id: &[u8; 32],
    key: ScVal,
    durability: ContractDataDurability,
) -> SentinelResult<String> {
    let ledger_key = LedgerKey::ContractData(LedgerKeyContractData {
        contract: ScAddress::Contract(Hash(*contract_id)),
        key,
        durability,
    });

    ledger_key.to_xdr_base64(Limits::none()).map_err(|e| {
        SentinelError::SerializationError(format!("Failed to encode contract data key: {}", e))
    })
}

/// Parses a Soroban symbol string into an `ScVal::Symbol`.
fn symbol_scval(symbol: &str) -> SentinelResult<ScVal> {
    let sc_symbol = ScSymbol(symbol.try_into().map_err(|e| {
        SentinelError::InvalidXdrKey(format!("Invalid symbol '{}': {}", symbol, e))
    })?);
    Ok(ScVal::Symbol(sc_symbol))
}

/// Decodes a base64-encoded `ScVal` key with bounded XDR limits.
fn decode_scval_key(scval_base64: &str) -> SentinelResult<ScVal> {
    ScVal::from_xdr_base64(scval_base64, safe_xdr_limits()).map_err(|e| {
        SentinelError::InvalidXdrKey(format!(
            "Failed to decode base64 ScVal key '{}': {}",
            scval_base64, e
        ))
    })
}

/// Derives the base64-encoded `LedgerKey` XDR for a persistent storage key given an `ScVal`.
pub fn derive_persistent_key_from_scval(
    contract_id: &[u8; 32],
    key: ScVal,
) -> SentinelResult<String> {
    derive_contract_data_key_from_scval(contract_id, key, ContractDataDurability::Persistent)
}

/// Derives the base64-encoded `LedgerKey` XDR for a persistent storage entry using a symbol string.
pub fn derive_persistent_symbol_key(
    contract_id: &[u8; 32],
    symbol: &str,
) -> SentinelResult<String> {
    derive_persistent_key_from_scval(contract_id, symbol_scval(symbol)?)
}

/// Derives the base64-encoded `LedgerKey` XDR for a persistent storage entry from a base64-encoded `ScVal`.
pub fn derive_persistent_xdr_key(
    contract_id: &[u8; 32],
    scval_base64: &str,
) -> SentinelResult<String> {
    derive_persistent_key_from_scval(contract_id, decode_scval_key(scval_base64)?)
}

/// Derives the base64-encoded `LedgerKey` XDR for a temporary storage key given an `ScVal`.
pub fn derive_temporary_key_from_scval(
    contract_id: &[u8; 32],
    key: ScVal,
) -> SentinelResult<String> {
    derive_contract_data_key_from_scval(contract_id, key, ContractDataDurability::Temporary)
}

/// Derives the base64-encoded `LedgerKey` XDR for a temporary storage entry using a symbol string.
pub fn derive_temporary_symbol_key(contract_id: &[u8; 32], symbol: &str) -> SentinelResult<String> {
    derive_temporary_key_from_scval(contract_id, symbol_scval(symbol)?)
}

/// Derives the base64-encoded `LedgerKey` XDR for a temporary storage entry from a base64-encoded `ScVal`.
pub fn derive_temporary_xdr_key(
    contract_id: &[u8; 32],
    scval_base64: &str,
) -> SentinelResult<String> {
    derive_temporary_key_from_scval(contract_id, decode_scval_key(scval_base64)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_contract_id_valid() {
        let expected_bytes = [42u8; 32];
        let valid_strkey = stellar_strkey::Contract(expected_bytes).to_string();
        let contract_id = parse_contract_id(&valid_strkey).expect("should parse valid contract ID");
        assert_eq!(contract_id, expected_bytes);
    }

    #[test]
    fn test_parse_contract_id_invalid() {
        let result = parse_contract_id("INVALID_STRKEY");
        assert!(matches!(
            result,
            Err(SentinelError::InvalidContractAddress(_))
        ));
    }

    #[test]
    fn test_derive_contract_instance_key_deterministic() {
        let contract_id = [1u8; 32];
        let key_base64 =
            derive_contract_instance_key(&contract_id).expect("should derive instance key");

        // Should be valid base64 and decode back to matching LedgerKey
        let decoded =
            LedgerKey::from_xdr_base64(&key_base64, Limits::none()).expect("should decode back");
        if let LedgerKey::ContractData(data) = decoded {
            assert_eq!(data.durability, ContractDataDurability::Persistent);
            assert_eq!(data.key, ScVal::LedgerKeyContractInstance);
            if let ScAddress::Contract(Hash(h)) = data.contract {
                assert_eq!(h, [1u8; 32]);
            } else {
                panic!("Expected ScAddress::Contract");
            }
        } else {
            panic!("Expected LedgerKey::ContractData");
        }
    }

    #[test]
    fn test_derive_contract_code_key_deterministic() {
        let wasm_hash = [2u8; 32];
        let key_base64 = derive_contract_code_key(&wasm_hash).expect("should derive code key");

        let decoded =
            LedgerKey::from_xdr_base64(&key_base64, Limits::none()).expect("should decode back");
        if let LedgerKey::ContractCode(code_key) = decoded {
            assert_eq!(code_key.hash.0, [2u8; 32]);
        } else {
            panic!("Expected LedgerKey::ContractCode");
        }
    }

    #[test]
    fn test_extract_wasm_hash_from_instance_entry() {
        use stellar_xdr::curr::{ContractDataEntry, ExtensionPoint, ScContractInstance};

        let wasm_hash = [7u8; 32];
        let contract_id = [9u8; 32];

        let entry_data = LedgerEntryData::ContractData(ContractDataEntry {
            ext: ExtensionPoint::V0,
            contract: ScAddress::Contract(Hash(contract_id)),
            key: ScVal::LedgerKeyContractInstance,
            durability: ContractDataDurability::Persistent,
            val: ScVal::ContractInstance(ScContractInstance {
                executable: ContractExecutable::Wasm(Hash(wasm_hash)),
                storage: None,
            }),
        });

        let base64 = entry_data.to_xdr_base64(Limits::none()).unwrap();
        let extracted = extract_wasm_hash_from_instance_entry(&base64).unwrap();
        assert_eq!(extracted, Some(wasm_hash));
    }

    #[test]
    fn test_extract_wasm_hash_stellar_asset() {
        use stellar_xdr::curr::{ContractDataEntry, ExtensionPoint, ScContractInstance};

        let contract_id = [9u8; 32];

        let entry_data = LedgerEntryData::ContractData(ContractDataEntry {
            ext: ExtensionPoint::V0,
            contract: ScAddress::Contract(Hash(contract_id)),
            key: ScVal::LedgerKeyContractInstance,
            durability: ContractDataDurability::Persistent,
            val: ScVal::ContractInstance(ScContractInstance {
                executable: ContractExecutable::StellarAsset,
                storage: None,
            }),
        });

        let base64 = entry_data.to_xdr_base64(Limits::none()).unwrap();
        let extracted = extract_wasm_hash_from_instance_entry(&base64).unwrap();
        assert_eq!(extracted, None);
    }

    #[test]
    fn test_derive_persistent_symbol_key_valid() {
        let contract_id = [5u8; 32];
        let key_base64 = derive_persistent_symbol_key(&contract_id, "Counter")
            .expect("should derive persistent symbol key");

        let decoded =
            LedgerKey::from_xdr_base64(&key_base64, Limits::none()).expect("should decode back");

        if let LedgerKey::ContractData(data) = decoded {
            assert_eq!(data.durability, ContractDataDurability::Persistent);
            if let ScVal::Symbol(sym) = data.key {
                assert_eq!(sym.0.to_string(), "Counter");
            } else {
                panic!("Expected ScVal::Symbol");
            }
        } else {
            panic!("Expected LedgerKey::ContractData");
        }
    }

    #[test]
    fn test_derive_persistent_symbol_key_too_long() {
        let contract_id = [5u8; 32];
        let long_symbol = "a".repeat(33); // max symbol length is 32 in Soroban
        let result = derive_persistent_symbol_key(&contract_id, &long_symbol);
        assert!(matches!(result, Err(SentinelError::InvalidXdrKey(_))));
    }

    #[test]
    fn test_derive_persistent_xdr_key_valid() {
        let contract_id = [6u8; 32];
        let raw_val = ScVal::U32(12345);
        let scval_base64 = raw_val.to_xdr_base64(Limits::none()).unwrap();

        let key_base64 = derive_persistent_xdr_key(&contract_id, &scval_base64)
            .expect("should derive persistent xdr key");

        let decoded =
            LedgerKey::from_xdr_base64(&key_base64, Limits::none()).expect("should decode back");

        if let LedgerKey::ContractData(data) = decoded {
            assert_eq!(data.durability, ContractDataDurability::Persistent);
            assert_eq!(data.key, ScVal::U32(12345));
        } else {
            panic!("Expected LedgerKey::ContractData");
        }
    }

    #[test]
    fn test_derive_persistent_xdr_key_invalid() {
        let contract_id = [6u8; 32];
        let result = derive_persistent_xdr_key(&contract_id, "invalid_base64_xdr!!!");
        assert!(matches!(result, Err(SentinelError::InvalidXdrKey(_))));
    }

    #[test]
    fn test_temporary_and_persistent_keys_differ_for_same_scval() {
        let contract_id = [8u8; 32];
        let scval = ScVal::Symbol(ScSymbol("NONCE".try_into().unwrap()));

        let persistent = derive_persistent_key_from_scval(&contract_id, scval.clone()).unwrap();
        let temporary = derive_temporary_key_from_scval(&contract_id, scval.clone()).unwrap();
        assert_ne!(persistent, temporary);

        let decoded = LedgerKey::from_xdr_base64(&temporary, Limits::none()).unwrap();
        let LedgerKey::ContractData(data) = decoded else {
            panic!("Expected LedgerKey::ContractData");
        };
        assert_eq!(data.durability, ContractDataDurability::Temporary);
        assert_eq!(data.key, scval);
    }

    #[test]
    fn test_derive_temporary_symbol_key_matches_scval_derivation() {
        let contract_id = [8u8; 32];
        let from_symbol = derive_temporary_symbol_key(&contract_id, "NONCE").unwrap();
        let from_scval = derive_temporary_key_from_scval(
            &contract_id,
            ScVal::Symbol(ScSymbol("NONCE".try_into().unwrap())),
        )
        .unwrap();
        assert_eq!(from_symbol, from_scval);
        assert_ne!(
            from_symbol,
            derive_persistent_symbol_key(&contract_id, "NONCE").unwrap()
        );
    }

    #[test]
    fn test_derive_temporary_xdr_key_valid_and_invalid() {
        let contract_id = [8u8; 32];
        let scval_base64 = ScVal::U64(7).to_xdr_base64(Limits::none()).unwrap();
        let key = derive_temporary_xdr_key(&contract_id, &scval_base64).unwrap();
        let LedgerKey::ContractData(data) =
            LedgerKey::from_xdr_base64(&key, Limits::none()).unwrap()
        else {
            panic!("Expected LedgerKey::ContractData");
        };
        assert_eq!(data.durability, ContractDataDurability::Temporary);
        assert_eq!(data.key, ScVal::U64(7));

        assert!(matches!(
            derive_temporary_xdr_key(&contract_id, "not-xdr!!"),
            Err(SentinelError::InvalidXdrKey(_))
        ));
        assert!(matches!(
            derive_temporary_symbol_key(&contract_id, &"a".repeat(33)),
            Err(SentinelError::InvalidXdrKey(_))
        ));
    }

    fn instance_with_storage(storage: Option<stellar_xdr::curr::ScMap>) -> String {
        use stellar_xdr::curr::{ContractDataEntry, ExtensionPoint, ScContractInstance};
        LedgerEntryData::ContractData(ContractDataEntry {
            ext: ExtensionPoint::V0,
            contract: ScAddress::Contract(Hash([3u8; 32])),
            key: ScVal::LedgerKeyContractInstance,
            durability: ContractDataDurability::Persistent,
            val: ScVal::ContractInstance(ScContractInstance {
                executable: ContractExecutable::Wasm(Hash([4u8; 32])),
                storage,
            }),
        })
        .to_xdr_base64(Limits::none())
        .unwrap()
    }

    fn storage_map(keys: u32, value_bytes: usize) -> stellar_xdr::curr::ScMap {
        use stellar_xdr::curr::{ScBytes, ScMap, ScMapEntry};
        let entries: Vec<ScMapEntry> = (0..keys)
            .map(|i| ScMapEntry {
                key: ScVal::U32(i),
                val: ScVal::Bytes(ScBytes(vec![0xAB; value_bytes].try_into().unwrap())),
            })
            .collect();
        ScMap(entries.try_into().unwrap())
    }

    #[test]
    fn test_instance_storage_stats_absent_empty_and_small_maps() {
        let absent = extract_instance_storage_stats(&instance_with_storage(None)).unwrap();
        assert_eq!(absent.key_count, 0);

        let empty = extract_instance_storage_stats(&instance_with_storage(Some(storage_map(0, 0))))
            .unwrap();
        assert_eq!(empty.key_count, 0);

        let small = extract_instance_storage_stats(&instance_with_storage(Some(storage_map(3, 8))))
            .unwrap();
        assert_eq!(small.key_count, 3);
        assert!(small.entry_size_bytes > empty.entry_size_bytes);
        assert!(empty.entry_size_bytes > absent.entry_size_bytes);
    }

    #[test]
    fn test_instance_storage_stats_measures_full_ledger_entry() {
        let data_xdr = instance_with_storage(Some(storage_map(2, 16)));
        let data = LedgerEntryData::from_xdr_base64(&data_xdr, Limits::none()).unwrap();
        let data_len = data.to_xdr(Limits::none()).unwrap().len();
        let entry_xdr = LedgerEntry {
            last_modified_ledger_seq: 77,
            data,
            ext: LedgerEntryExt::V0,
        }
        .to_xdr_base64(Limits::none())
        .unwrap();

        let from_data = extract_instance_storage_stats(&data_xdr).unwrap();
        let from_entry = extract_instance_storage_stats(&entry_xdr).unwrap();
        // Bare entry data is measured as the stored LedgerEntry: +4 bytes sequence, +4 bytes ext.
        assert_eq!(from_data.entry_size_bytes, data_len + 8);
        assert_eq!(from_data, from_entry);
    }

    #[test]
    fn test_instance_storage_stats_near_limit_map() {
        // ~64 KiB of storage values: close to the mainnet 64 KiB entry limit.
        let stats =
            extract_instance_storage_stats(&instance_with_storage(Some(storage_map(64, 1_000))))
                .unwrap();
        assert_eq!(stats.key_count, 64);
        assert!(stats.entry_size_bytes > 64 * 1_000);
        assert!(stats.entry_size_bytes < 70_000);
    }

    #[test]
    fn test_instance_storage_stats_rejects_malformed_and_non_instance_entries() {
        assert!(matches!(
            extract_instance_storage_stats("not-valid-xdr!!"),
            Err(SentinelError::InvalidXdrKey(_))
        ));
        let code_entry = LedgerEntryData::ContractCode(stellar_xdr::curr::ContractCodeEntry {
            ext: stellar_xdr::curr::ContractCodeEntryExt::V0,
            hash: Hash([1u8; 32]),
            code: vec![0u8; 4].try_into().unwrap(),
        })
        .to_xdr_base64(Limits::none())
        .unwrap();
        assert!(extract_instance_storage_stats(&code_entry).is_err());

        let plain_data = LedgerEntryData::ContractData(stellar_xdr::curr::ContractDataEntry {
            ext: stellar_xdr::curr::ExtensionPoint::V0,
            contract: ScAddress::Contract(Hash([1u8; 32])),
            key: ScVal::U32(1),
            durability: ContractDataDurability::Persistent,
            val: ScVal::U32(2),
        })
        .to_xdr_base64(Limits::none())
        .unwrap();
        assert!(extract_instance_storage_stats(&plain_data).is_err());
    }

    #[test]
    fn test_contract_data_entry_size_limit_key_and_extraction() {
        let key = derive_contract_data_entry_size_limit_key().unwrap();
        assert_eq!(
            LedgerKey::from_xdr_base64(&key, Limits::none()).unwrap(),
            LedgerKey::ConfigSetting(LedgerKeyConfigSetting {
                config_setting_id: ConfigSettingId::ContractDataEntrySizeBytes,
            })
        );

        let entry =
            LedgerEntryData::ConfigSetting(ConfigSettingEntry::ContractDataEntrySizeBytes(65_536))
                .to_xdr_base64(Limits::none())
                .unwrap();
        assert_eq!(
            extract_contract_data_entry_size_limit(&entry).unwrap(),
            65_536
        );

        let other = LedgerEntryData::ConfigSetting(ConfigSettingEntry::ContractMaxSizeBytes(1))
            .to_xdr_base64(Limits::none())
            .unwrap();
        assert!(extract_contract_data_entry_size_limit(&other).is_err());
        assert!(extract_contract_data_entry_size_limit("garbage").is_err());
    }

    #[test]
    fn test_safe_xdr_limits_configuration() {
        let limits = safe_xdr_limits();
        assert_eq!(limits.depth, 100);
        assert_eq!(limits.len, 10 * 1024 * 1024);
    }

    #[test]
    fn test_extract_wasm_hash_malformed_rejected() {
        let result = extract_wasm_hash_from_instance_entry("not-valid-xdr-base64!!");
        assert!(matches!(result, Err(SentinelError::InvalidXdrKey(_))));
    }
}
