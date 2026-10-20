//! Deterministic XDR key derivation adapter using `stellar-xdr` and `stellar-strkey`.

use std::str::FromStr;
use stellar_strkey::Contract;
use stellar_xdr::curr::{
    ContractDataDurability, ContractExecutable, Hash, LedgerEntry, LedgerEntryData, LedgerKey,
    LedgerKeyContractCode, LedgerKeyContractData, Limits, ReadXdr, ScAddress, ScVal, WriteXdr,
};

use crate::error::{SentinelError, SentinelResult};

/// Parses a Stellar Strkey contract address (e.g., `CA3D...`) into a 32-byte contract ID.
pub fn parse_contract_id(strkey: &str) -> SentinelResult<[u8; 32]> {
    Contract::from_str(strkey)
        .map(|c| c.0)
        .map_err(|e| SentinelError::InvalidContractAddress(format!("{}: {}", strkey, e)))
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
        if let Ok(entry) = LedgerEntry::from_xdr_base64(entry_xdr_base64, Limits::none()) {
            entry.data
        } else {
            LedgerEntryData::from_xdr_base64(entry_xdr_base64, Limits::none()).map_err(|e| {
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
}
