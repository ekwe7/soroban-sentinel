//! Deterministic XDR key derivation adapter using `stellar-xdr` and `stellar-strkey`.

use std::str::FromStr;
use stellar_strkey::Contract;
use stellar_xdr::curr::{
    ContractDataDurability, ContractExecutable, Hash, LedgerEntry, LedgerEntryData, LedgerKey,
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

/// Derives the base64-encoded `LedgerKey` XDR for a persistent storage key given an `ScVal`.
pub fn derive_persistent_key_from_scval(
    contract_id: &[u8; 32],
    key: ScVal,
) -> SentinelResult<String> {
    let ledger_key = LedgerKey::ContractData(LedgerKeyContractData {
        contract: ScAddress::Contract(Hash(*contract_id)),
        key,
        durability: ContractDataDurability::Persistent,
    });

    ledger_key.to_xdr_base64(Limits::none()).map_err(|e| {
        SentinelError::SerializationError(format!("Failed to encode persistent key: {}", e))
    })
}

/// Derives the base64-encoded `LedgerKey` XDR for a persistent storage entry using a symbol string.
pub fn derive_persistent_symbol_key(
    contract_id: &[u8; 32],
    symbol: &str,
) -> SentinelResult<String> {
    let sc_symbol = ScSymbol(symbol.try_into().map_err(|e| {
        SentinelError::InvalidXdrKey(format!("Invalid symbol '{}': {}", symbol, e))
    })?);
    derive_persistent_key_from_scval(contract_id, ScVal::Symbol(sc_symbol))
}

/// Derives the base64-encoded `LedgerKey` XDR for a persistent storage entry from a base64-encoded `ScVal`.
pub fn derive_persistent_xdr_key(
    contract_id: &[u8; 32],
    scval_base64: &str,
) -> SentinelResult<String> {
    let key_scval = ScVal::from_xdr_base64(scval_base64, safe_xdr_limits()).map_err(|e| {
        SentinelError::InvalidXdrKey(format!(
            "Failed to decode base64 ScVal key '{}': {}",
            scval_base64, e
        ))
    })?;
    derive_persistent_key_from_scval(contract_id, key_scval)
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
