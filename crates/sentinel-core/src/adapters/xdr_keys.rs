//! Deterministic XDR key derivation adapter using `stellar-xdr` and `stellar-strkey`.

use std::str::FromStr;
use stellar_strkey::{Contract, Strkey};
use stellar_xdr::curr::{
    AccountId, ContractDataDurability, ContractExecutable, Hash, LedgerEntry, LedgerEntryData,
    LedgerKey, LedgerKeyContractCode, LedgerKeyContractData, Limits, PublicKey, ReadXdr, ScAddress,
    ScMap, ScMapEntry, ScSymbol, ScVal, ScVec, Uint256, WriteXdr,
};

use crate::domain::{InstanceStorageBreakdown, SacAssetInfo};
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
#[tracing::instrument(skip(entry_xdr_base64), fields(xdr.base64_len = entry_xdr_base64.len()))]
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

/// Derives the base64-encoded `LedgerKey` XDR for a temporary storage entry using an `ScVal`.
pub fn derive_temporary_key_from_scval(
    contract_id: &[u8; 32],
    key: ScVal,
) -> SentinelResult<String> {
    let ledger_key = LedgerKey::ContractData(LedgerKeyContractData {
        contract: ScAddress::Contract(Hash(*contract_id)),
        key,
        durability: ContractDataDurability::Temporary,
    });

    ledger_key.to_xdr_base64(Limits::none()).map_err(|e| {
        SentinelError::SerializationError(format!("Failed to encode temporary key: {}", e))
    })
}

/// Derives the base64-encoded `LedgerKey` XDR for a temporary storage entry using a symbol string.
pub fn derive_temporary_symbol_key(contract_id: &[u8; 32], symbol: &str) -> SentinelResult<String> {
    let sc_symbol = ScSymbol(symbol.try_into().map_err(|e| {
        SentinelError::InvalidXdrKey(format!("Invalid symbol '{}': {}", symbol, e))
    })?);
    derive_temporary_key_from_scval(contract_id, ScVal::Symbol(sc_symbol))
}

/// Validates and parses a Stellar strkey address (`G...` account or `C...` contract) into an `ScAddress`.
///
/// Returns typed `SentinelError::InvalidContractAddress` diagnostics on malformed or unsupported address types.
pub fn parse_sc_address(strkey: &str) -> SentinelResult<ScAddress> {
    let trimmed = strkey.trim();
    if trimmed.is_empty() {
        return Err(SentinelError::InvalidContractAddress(
            "Address string cannot be empty".to_string(),
        ));
    }
    let parsed = Strkey::from_string(trimmed).map_err(|e| {
        SentinelError::InvalidContractAddress(format!(
            "Invalid strkey address '{}': {}",
            trimmed, e
        ))
    })?;
    match parsed {
        Strkey::PublicKeyEd25519(pk) => {
            let account_id = AccountId(PublicKey::PublicKeyTypeEd25519(Uint256(pk.0)));
            Ok(ScAddress::Account(account_id))
        }
        Strkey::Contract(c) => Ok(ScAddress::Contract(Hash(c.0))),
        other => Err(SentinelError::InvalidContractAddress(format!(
            "Unsupported strkey address '{}': expected Account (G...) or Contract (C...), found {:?}",
            trimmed, other
        ))),
    }
}

/// Derives the base64-encoded `LedgerKey` XDR for a Stellar Asset Contract (SAC) balance entry (`DataKey::Balance(Address)`).
pub fn derive_sac_balance_key(
    contract_id: &[u8; 32],
    holder_address: &str,
) -> SentinelResult<String> {
    let holder_sc_addr = parse_sc_address(holder_address)?;
    let balance_sym = ScSymbol("Balance".try_into().map_err(|e| {
        SentinelError::SerializationError(format!("Failed to create Balance symbol: {}", e))
    })?);
    let elements = vec![ScVal::Symbol(balance_sym), ScVal::Address(holder_sc_addr)];
    let sc_vec: ScVec = elements.try_into().map_err(|e| {
        SentinelError::SerializationError(format!("Failed to create ScVec for Balance key: {}", e))
    })?;
    let key_scval = ScVal::Vec(Some(sc_vec));
    derive_persistent_key_from_scval(contract_id, key_scval)
}

/// Derives the base64-encoded `LedgerKey` XDR for a Stellar Asset Contract (SAC) allowance entry (`DataKey::Allowance(AllowanceDataKey)`).
pub fn derive_sac_allowance_key(
    contract_id: &[u8; 32],
    from_address: &str,
    spender_address: &str,
) -> SentinelResult<String> {
    let from_sc_addr = parse_sc_address(from_address)?;
    let spender_sc_addr = parse_sc_address(spender_address)?;

    let allowance_sym = ScSymbol("Allowance".try_into().map_err(|e| {
        SentinelError::SerializationError(format!("Failed to create Allowance symbol: {}", e))
    })?);

    // In Soroban token contracttype, AllowanceDataKey is a struct { from: Address, spender: Address }
    // which encodes as an ScMap with sorted keys: "from", then "spender".
    let from_sym = ScSymbol("from".try_into().map_err(|e| {
        SentinelError::SerializationError(format!("Failed to create 'from' symbol: {}", e))
    })?);
    let spender_sym = ScSymbol("spender".try_into().map_err(|e| {
        SentinelError::SerializationError(format!("Failed to create 'spender' symbol: {}", e))
    })?);

    let map_entries = vec![
        ScMapEntry {
            key: ScVal::Symbol(from_sym),
            val: ScVal::Address(from_sc_addr),
        },
        ScMapEntry {
            key: ScVal::Symbol(spender_sym),
            val: ScVal::Address(spender_sc_addr),
        },
    ];
    let sc_map = ScMap(map_entries.try_into().map_err(|e| {
        SentinelError::SerializationError(format!(
            "Failed to create ScMap for AllowanceDataKey: {}",
            e
        ))
    })?);

    let elements = vec![ScVal::Symbol(allowance_sym), ScVal::Map(Some(sc_map))];
    let sc_vec: ScVec = elements.try_into().map_err(|e| {
        SentinelError::SerializationError(format!(
            "Failed to create ScVec for Allowance key: {}",
            e
        ))
    })?;
    let key_scval = ScVal::Vec(Some(sc_vec));
    derive_persistent_key_from_scval(contract_id, key_scval)
}

/// Produces a human-readable summary of an `ScVal` key.
pub fn summarize_scval(scval: &ScVal) -> String {
    match scval {
        ScVal::Symbol(sym) => sym.0.to_string(),
        ScVal::String(s) => s.0.to_string(),
        ScVal::U32(n) => n.to_string(),
        ScVal::I32(n) => n.to_string(),
        ScVal::U64(n) => n.to_string(),
        ScVal::I64(n) => n.to_string(),
        ScVal::Bool(b) => b.to_string(),
        ScVal::Address(addr) => match addr {
            ScAddress::Account(acc) => {
                let AccountId(PublicKey::PublicKeyTypeEd25519(Uint256(raw))) = acc;
                stellar_strkey::ed25519::PublicKey(*raw).to_string()
            }
            ScAddress::Contract(Hash(hash)) => stellar_strkey::Contract(*hash).to_string(),
        },
        ScVal::Vec(Some(vec)) => {
            let items: Vec<String> = vec.0.iter().map(summarize_scval).collect();
            format!("[{}]", items.join(", "))
        }
        ScVal::Map(Some(map)) => {
            let entries: Vec<String> = map
                .0
                .iter()
                .map(|e| format!("{}: {}", summarize_scval(&e.key), summarize_scval(&e.val)))
                .collect();
            format!("{{{}}}", entries.join(", "))
        }
        ScVal::Bytes(b) => format!("bytes[{}]", b.0.len()),
        ScVal::Void => "void".to_string(),
        other => format!("{:?}", other),
    }
}

/// Extracts a structured breakdown of each key/value entry in `ScContractInstance.storage`.
///
/// Returns a list of [`InstanceStorageBreakdown`] entries sorted descending by total byte size.
pub fn extract_instance_storage_breakdown(
    entry_xdr_base64: &str,
) -> SentinelResult<Vec<InstanceStorageBreakdown>> {
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
            let Some(storage_map) = instance.storage else {
                return Ok(Vec::new());
            };

            let mut breakdowns = Vec::with_capacity(storage_map.0.len());
            for entry in storage_map.0.iter() {
                let key_bytes = entry
                    .key
                    .to_xdr(safe_xdr_limits())
                    .map_err(|e| {
                        SentinelError::SerializationError(format!(
                            "Failed to serialize storage map key ScVal: {}",
                            e
                        ))
                    })?
                    .len();

                let val_bytes = entry
                    .val
                    .to_xdr(safe_xdr_limits())
                    .map_err(|e| {
                        SentinelError::SerializationError(format!(
                            "Failed to serialize storage map val ScVal: {}",
                            e
                        ))
                    })?
                    .len();

                let key_summary = summarize_scval(&entry.key);
                breakdowns.push(InstanceStorageBreakdown::new(
                    key_summary,
                    key_bytes,
                    val_bytes,
                ));
            }

            // Sort breakdown entries by total byte size descending for operator readability
            breakdowns.sort_by_key(|a| std::cmp::Reverse(a.total_bytes));
            Ok(breakdowns)
        } else {
            Ok(Vec::new())
        }
    } else {
        Err(SentinelError::InvalidXdrKey(
            "Decoded ledger entry is not ContractData".to_string(),
        ))
    }
}

/// Helper to extract string from `ScVal::String`, `ScVal::Symbol`, or ASCII `ScVal::Bytes`.
fn scval_to_string_lossy(val: &ScVal) -> Option<String> {
    match val {
        ScVal::String(s) => Some(s.0.to_string()),
        ScVal::Symbol(sym) => Some(sym.0.to_string()),
        ScVal::Bytes(b) => std::str::from_utf8(&b.0).ok().map(|s| s.to_string()),
        _ => None,
    }
}

/// Helper to convert `ScVal::Address` into a Strkey string.
fn scval_to_address_strkey(val: &ScVal) -> Option<String> {
    if let ScVal::Address(addr) = val {
        match addr {
            ScAddress::Account(acc) => {
                let AccountId(PublicKey::PublicKeyTypeEd25519(Uint256(raw))) = acc;
                Some(stellar_strkey::ed25519::PublicKey(*raw).to_string())
            }
            ScAddress::Contract(Hash(hash)) => Some(stellar_strkey::Contract(*hash).to_string()),
        }
    } else {
        None
    }
}

/// Extracts SAC [`SacAssetInfo`] (asset name/symbol/code/issuer) from an instance entry's XDR data.
///
/// Returns `Ok(Some(SacAssetInfo))` if the contract is a Stellar Asset Contract,
/// or `Ok(None)` if it is a standard WASM contract.
pub fn extract_sac_asset_info(entry_xdr_base64: &str) -> SentinelResult<Option<SacAssetInfo>> {
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
                ContractExecutable::Wasm(_) => Ok(None),
                ContractExecutable::StellarAsset => {
                    let mut asset_code = None;
                    let mut issuer = None;
                    let mut symbol = None;
                    let mut name = None;
                    let mut decimals = Some(7);
                    let mut is_native = false;

                    if let Some(storage_map) = instance.storage {
                        for entry in storage_map.0.iter() {
                            let key_str = match &entry.key {
                                ScVal::Symbol(sym) => sym.0.to_string(),
                                ScVal::String(s) => s.0.to_string(),
                                _ => summarize_scval(&entry.key),
                            };

                            let key_lower = key_str.to_ascii_lowercase();

                            if key_lower == "assetinfo" || key_lower == "asset_info" {
                                match &entry.val {
                                    ScVal::Vec(Some(vec)) => {
                                        if let Some(first) = vec.0.first() {
                                            let first_str = scval_to_string_lossy(first)
                                                .unwrap_or_default()
                                                .to_ascii_lowercase();
                                            if first_str == "native" {
                                                is_native = true;
                                                asset_code = Some("native".to_string());
                                                symbol = Some("XLM".to_string());
                                                name = Some("Native".to_string());
                                            } else if first_str == "alphanum4"
                                                || first_str == "alphanum12"
                                            {
                                                if vec.0.len() >= 2 {
                                                    asset_code = scval_to_string_lossy(&vec.0[1]);
                                                    symbol = asset_code.clone();
                                                }
                                                if vec.0.len() >= 3 {
                                                    issuer = scval_to_address_strkey(&vec.0[2]);
                                                }
                                            }
                                        }
                                    }
                                    ScVal::Map(Some(map)) => {
                                        for m in map.0.iter() {
                                            let mk = scval_to_string_lossy(&m.key)
                                                .unwrap_or_default()
                                                .to_ascii_lowercase();
                                            if mk == "code" || mk == "asset_code" {
                                                asset_code = scval_to_string_lossy(&m.val);
                                            } else if mk == "issuer" {
                                                issuer = scval_to_address_strkey(&m.val)
                                                    .or_else(|| scval_to_string_lossy(&m.val));
                                            } else if mk == "symbol" {
                                                symbol = scval_to_string_lossy(&m.val);
                                            } else if mk == "name" {
                                                name = scval_to_string_lossy(&m.val);
                                            } else if mk == "decimals" {
                                                if let ScVal::U32(d) = m.val {
                                                    decimals = Some(d);
                                                }
                                            }
                                        }
                                    }
                                    _ => {}
                                }
                            } else if key_lower == "metadata" {
                                if let ScVal::Map(Some(map)) = &entry.val {
                                    for m in map.0.iter() {
                                        let mk = scval_to_string_lossy(&m.key)
                                            .unwrap_or_default()
                                            .to_ascii_lowercase();
                                        if mk == "name" {
                                            name = scval_to_string_lossy(&m.val);
                                        } else if mk == "symbol" {
                                            symbol = scval_to_string_lossy(&m.val);
                                        } else if mk == "decimal" || mk == "decimals" {
                                            if let ScVal::U32(d) = m.val {
                                                decimals = Some(d);
                                            }
                                        }
                                    }
                                }
                            } else if key_lower == "symbol" {
                                symbol = scval_to_string_lossy(&entry.val);
                            } else if key_lower == "name" {
                                name = scval_to_string_lossy(&entry.val);
                            } else if key_lower == "decimal" || key_lower == "decimals" {
                                if let ScVal::U32(d) = entry.val {
                                    decimals = Some(d);
                                }
                            }
                        }
                    }

                    // If no explicit asset info was parsed, default to native SAC
                    if asset_code.is_none() && symbol.is_none() && !is_native {
                        is_native = true;
                        asset_code = Some("native".to_string());
                        symbol = Some("XLM".to_string());
                        name = Some("Native".to_string());
                    }

                    Ok(Some(SacAssetInfo::new(
                        asset_code, issuer, symbol, name, decimals, is_native,
                    )))
                }
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

    #[test]
    fn test_parse_sc_address_account_and_contract() {
        let account_bytes = [15u8; 32];
        let account_strkey = stellar_strkey::ed25519::PublicKey(account_bytes).to_string();
        let sc_addr = parse_sc_address(&account_strkey).expect("should parse valid G... address");
        if let ScAddress::Account(AccountId(PublicKey::PublicKeyTypeEd25519(Uint256(raw)))) =
            sc_addr
        {
            assert_eq!(raw, account_bytes);
        } else {
            panic!("Expected ScAddress::Account");
        }

        let contract_bytes = [16u8; 32];
        let contract_strkey = stellar_strkey::Contract(contract_bytes).to_string();
        let sc_addr = parse_sc_address(&contract_strkey).expect("should parse valid C... address");
        if let ScAddress::Contract(Hash(raw)) = sc_addr {
            assert_eq!(raw, contract_bytes);
        } else {
            panic!("Expected ScAddress::Contract");
        }
    }

    #[test]
    fn test_parse_sc_address_invalid() {
        let empty_err = parse_sc_address("").unwrap_err();
        assert!(matches!(
            empty_err,
            SentinelError::InvalidContractAddress(_)
        ));

        let bad_strkey = parse_sc_address("G_INVALID_CHECKSUM_NOT_STRKEY").unwrap_err();
        assert!(matches!(
            bad_strkey,
            SentinelError::InvalidContractAddress(_)
        ));
    }

    #[test]
    fn test_derive_sac_balance_key_deterministic() {
        let contract_id = [21u8; 32];
        let holder_bytes = [22u8; 32];
        let holder_strkey = stellar_strkey::ed25519::PublicKey(holder_bytes).to_string();

        let key_base64 = derive_sac_balance_key(&contract_id, &holder_strkey).unwrap();
        let decoded = LedgerKey::from_xdr_base64(&key_base64, Limits::none()).unwrap();

        if let LedgerKey::ContractData(data) = decoded {
            assert_eq!(data.durability, ContractDataDurability::Persistent);
            if let ScVal::Vec(Some(vec)) = data.key {
                assert_eq!(vec.0.len(), 2);
                if let ScVal::Symbol(sym) = &vec.0[0] {
                    assert_eq!(sym.0.to_string(), "Balance");
                } else {
                    panic!("Expected Balance symbol");
                }
            } else {
                panic!("Expected ScVal::Vec for SAC balance key");
            }
        } else {
            panic!("Expected LedgerKey::ContractData");
        }
    }

    #[test]
    fn test_derive_sac_allowance_key_deterministic() {
        let contract_id = [21u8; 32];
        let from_bytes = [22u8; 32];
        let from_strkey = stellar_strkey::ed25519::PublicKey(from_bytes).to_string();
        let spender_bytes = [23u8; 32];
        let spender_strkey = stellar_strkey::Contract(spender_bytes).to_string();

        let key_base64 =
            derive_sac_allowance_key(&contract_id, &from_strkey, &spender_strkey).unwrap();
        let decoded = LedgerKey::from_xdr_base64(&key_base64, Limits::none()).unwrap();

        if let LedgerKey::ContractData(data) = decoded {
            assert_eq!(data.durability, ContractDataDurability::Persistent);
            if let ScVal::Vec(Some(vec)) = data.key {
                assert_eq!(vec.0.len(), 2);
                if let ScVal::Symbol(sym) = &vec.0[0] {
                    assert_eq!(sym.0.to_string(), "Allowance");
                } else {
                    panic!("Expected Allowance symbol");
                }
                if let ScVal::Map(Some(map)) = &vec.0[1] {
                    assert_eq!(map.0.len(), 2);
                    assert_eq!(summarize_scval(&map.0[0].key), "from");
                    assert_eq!(summarize_scval(&map.0[1].key), "spender");
                } else {
                    panic!("Expected ScVal::Map for AllowanceDataKey");
                }
            } else {
                panic!("Expected ScVal::Vec for SAC allowance key");
            }
        } else {
            panic!("Expected LedgerKey::ContractData");
        }
    }

    #[test]
    fn test_extract_instance_storage_breakdown_multi_key_and_sort() {
        use stellar_xdr::curr::{ContractDataEntry, ExtensionPoint, ScContractInstance};

        let contract_id = [30u8; 32];
        let key1 = ScVal::Symbol(ScSymbol("SmallKey".try_into().unwrap()));
        let val1 = ScVal::U32(10); // short value

        let key2 = ScVal::Symbol(ScSymbol("LargeKey".try_into().unwrap()));
        let val2 = ScVal::Bytes(vec![1u8; 128].try_into().unwrap());

        let storage_map = ScMap(
            vec![
                ScMapEntry {
                    key: key1,
                    val: val1,
                },
                ScMapEntry {
                    key: key2,
                    val: val2,
                },
            ]
            .try_into()
            .unwrap(),
        );

        let entry_data = LedgerEntryData::ContractData(ContractDataEntry {
            ext: ExtensionPoint::V0,
            contract: ScAddress::Contract(Hash(contract_id)),
            key: ScVal::LedgerKeyContractInstance,
            durability: ContractDataDurability::Persistent,
            val: ScVal::ContractInstance(ScContractInstance {
                executable: ContractExecutable::StellarAsset,
                storage: Some(storage_map),
            }),
        });

        let base64 = entry_data.to_xdr_base64(Limits::none()).unwrap();
        let breakdown = extract_instance_storage_breakdown(&base64).unwrap();

        assert_eq!(breakdown.len(), 2);
        // Sorted descending by total_bytes: LargeKey should be first
        assert_eq!(breakdown[0].key_summary, "LargeKey");
        assert_eq!(breakdown[1].key_summary, "SmallKey");
        assert!(breakdown[0].total_bytes > breakdown[1].total_bytes);
    }

    #[test]
    fn test_extract_instance_storage_breakdown_empty_or_none() {
        use stellar_xdr::curr::{ContractDataEntry, ExtensionPoint, ScContractInstance};

        let contract_id = [30u8; 32];
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
        let breakdown = extract_instance_storage_breakdown(&base64).unwrap();
        assert!(breakdown.is_empty());
    }

    #[test]
    fn test_extract_sac_asset_info_alphanum4() {
        use stellar_xdr::curr::{ContractDataEntry, ExtensionPoint, ScContractInstance};

        let contract_id = [35u8; 32];
        let issuer_bytes = [36u8; 32];
        let issuer_addr = ScAddress::Account(AccountId(PublicKey::PublicKeyTypeEd25519(Uint256(
            issuer_bytes,
        ))));
        let issuer_strkey = stellar_strkey::ed25519::PublicKey(issuer_bytes).to_string();

        let asset_info_key = ScVal::Symbol(ScSymbol("AssetInfo".try_into().unwrap()));
        let asset_info_val = ScVal::Vec(Some(ScVec(
            vec![
                ScVal::Symbol(ScSymbol("AlphaNum4".try_into().unwrap())),
                ScVal::Symbol(ScSymbol("USDC".try_into().unwrap())),
                ScVal::Address(issuer_addr),
            ]
            .try_into()
            .unwrap(),
        )));

        let storage_map = ScMap(
            vec![ScMapEntry {
                key: asset_info_key,
                val: asset_info_val,
            }]
            .try_into()
            .unwrap(),
        );

        let entry_data = LedgerEntryData::ContractData(ContractDataEntry {
            ext: ExtensionPoint::V0,
            contract: ScAddress::Contract(Hash(contract_id)),
            key: ScVal::LedgerKeyContractInstance,
            durability: ContractDataDurability::Persistent,
            val: ScVal::ContractInstance(ScContractInstance {
                executable: ContractExecutable::StellarAsset,
                storage: Some(storage_map),
            }),
        });

        let base64 = entry_data.to_xdr_base64(Limits::none()).unwrap();
        let info = extract_sac_asset_info(&base64)
            .unwrap()
            .expect("should extract SAC info");

        assert_eq!(info.asset_code, Some("USDC".to_string()));
        assert_eq!(info.issuer, Some(issuer_strkey));
        assert!(!info.is_native);
    }

    #[test]
    fn test_extract_sac_asset_info_wasm_returns_none() {
        use stellar_xdr::curr::{ContractDataEntry, ExtensionPoint, ScContractInstance};

        let entry_data = LedgerEntryData::ContractData(ContractDataEntry {
            ext: ExtensionPoint::V0,
            contract: ScAddress::Contract(Hash([1u8; 32])),
            key: ScVal::LedgerKeyContractInstance,
            durability: ContractDataDurability::Persistent,
            val: ScVal::ContractInstance(ScContractInstance {
                executable: ContractExecutable::Wasm(Hash([2u8; 32])),
                storage: None,
            }),
        });

        let base64 = entry_data.to_xdr_base64(Limits::none()).unwrap();
        let info = extract_sac_asset_info(&base64).unwrap();
        assert!(info.is_none());
    }
}
