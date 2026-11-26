//! Deterministic XDR key derivation adapter using `stellar-xdr` and `stellar-strkey`.

use std::str::FromStr;
use stellar_strkey::Contract;
use stellar_xdr::curr::{
    AccountId, ConfigSettingEntry, ConfigSettingId, ContractDataDurability, ContractExecutable,
    Hash, LedgerEntry, LedgerEntryData, LedgerEntryExt, LedgerKey, LedgerKeyConfigSetting,
    LedgerKeyContractCode, LedgerKeyContractData, Limits, PublicKey, ReadXdr, ScAddress, ScMap,
    ScMapEntry, ScSymbol, ScVal, ScVec, Uint256, WriteXdr,
};

use super::key_expr::parse_key_expr;
use super::sc_address::parse_sc_address;
use crate::domain::{
    parse_key_pattern, InstanceStorageBreakdown, KeyPattern, KeyPatternCandidateInput,
    KeyPatternToken, SacAssetInfo, MAX_KEY_PATTERN_CANDIDATES,
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

/// Extracts the byte length of a compiled contract's WASM payload from its ledger-entry XDR.
pub fn extract_contract_code_size(entry_xdr_base64: &str) -> SentinelResult<usize> {
    let entry_data =
        if let Ok(entry) = LedgerEntry::from_xdr_base64(entry_xdr_base64, safe_xdr_limits()) {
            entry.data
        } else {
            LedgerEntryData::from_xdr_base64(entry_xdr_base64, safe_xdr_limits()).map_err(|e| {
                SentinelError::InvalidXdrKey(format!(
                    "Failed to decode base64 contract code XDR: {}",
                    e
                ))
            })?
        };

    match entry_data {
        LedgerEntryData::ContractCode(code) => Ok(code.code.as_slice().len()),
        _ => Err(SentinelError::InvalidXdrKey(
            "Decoded ledger entry is not ContractCode".to_string(),
        )),
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

/// Derives a contract data ledger key for the requested durability.
pub fn derive_contract_data_key(
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

/// Alias for derive_contract_data_key
pub fn derive_contract_data_key_from_scval(
    contract_id: &[u8; 32],
    key: ScVal,
    durability: ContractDataDurability,
) -> SentinelResult<String> {
    derive_contract_data_key(contract_id, key, durability)
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
    derive_contract_data_key(contract_id, key, ContractDataDurability::Persistent)
}

/// Derives the base64-encoded `LedgerKey` XDR for a temporary storage key given an `ScVal`.
pub fn derive_temporary_key_from_scval(
    contract_id: &[u8; 32],
    key: ScVal,
) -> SentinelResult<String> {
    derive_contract_data_key(contract_id, key, ContractDataDurability::Temporary)
}

/// Derives the base64-encoded `LedgerKey` XDR for a persistent storage entry using a symbol string.
pub fn derive_persistent_symbol_key(
    contract_id: &[u8; 32],
    symbol: &str,
) -> SentinelResult<String> {
    derive_persistent_key_from_scval(contract_id, symbol_scval(symbol)?)
}

/// Derives the base64-encoded `LedgerKey` XDR for a temporary storage key using a symbol.
pub fn derive_temporary_symbol_key(contract_id: &[u8; 32], symbol: &str) -> SentinelResult<String> {
    let sc_symbol = ScSymbol(symbol.try_into().map_err(|e| {
        SentinelError::InvalidXdrKey(format!("Invalid symbol '{}': {}", symbol, e))
    })?);
    derive_temporary_key_from_scval(contract_id, ScVal::Symbol(sc_symbol))
}

/// Derives the base64-encoded `LedgerKey` XDR for a persistent storage entry from a base64-encoded `ScVal`.
pub fn derive_persistent_xdr_key(
    contract_id: &[u8; 32],
    scval_base64: &str,
) -> SentinelResult<String> {
    derive_persistent_key_from_scval(contract_id, decode_scval_key(scval_base64)?)
}

/// Derives the base64-encoded `LedgerKey` XDR for a temporary storage entry from a base64-encoded `ScVal`.
pub fn derive_temporary_xdr_key(
    contract_id: &[u8; 32],
    scval_base64: &str,
) -> SentinelResult<String> {
    derive_temporary_key_from_scval(contract_id, decode_scval_key(scval_base64)?)
}

/// Derives the base64 XDR key for the network StateArchival config setting.
pub fn derive_state_archival_config_key() -> SentinelResult<String> {
    config_setting_key(ConfigSettingId::StateArchival)
}

/// Derives the base64 XDR key for the network ContractDataEntrySizeBytes setting.
pub fn derive_contract_data_entry_size_config_key() -> SentinelResult<String> {
    config_setting_key(ConfigSettingId::ContractDataEntrySizeBytes)
}

fn config_setting_key(setting_id: ConfigSettingId) -> SentinelResult<String> {
    LedgerKey::ConfigSetting(LedgerKeyConfigSetting {
        config_setting_id: setting_id,
    })
    .to_xdr_base64(Limits::none())
    .map_err(|error| {
        SentinelError::SerializationError(format!("Failed to encode config setting key: {error}"))
    })
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

/// Bounded key-pattern matcher. Wildcards expand only from caller-provided known inputs;
/// this never performs on-chain key enumeration.
#[derive(Debug, Clone)]
pub struct KeyPatternMatcher;

impl KeyPatternMatcher {
    /// Expands a key pattern into deterministic `LedgerKey` candidates.
    ///
    /// Composite patterns generate the full cross-product of their token
    /// candidates (addresses × symbol suffixes). Results are sorted by encoded
    /// XDR bytes, deduplicated, and capped at [`MAX_KEY_PATTERN_CANDIDATES`].
    pub fn expand(
        contract_id: &[u8; 32],
        pattern: &str,
        known: &KeyPatternCandidateInput,
    ) -> SentinelResult<Vec<LedgerKey>> {
        let parsed = parse_key_pattern(pattern).map_err(|(position, message)| {
            SentinelError::InvalidXdrKey(format!(
                "Key pattern syntax error at position {position}: {message}"
            ))
        })?;

        let values = match parsed {
            KeyPattern::Single(token) => Self::expand_token(&token, known)?,
            KeyPattern::Composite(tokens) => {
                Self::expand_token(&KeyPatternToken::Composite(tokens), known)?
            }
        };

        let mut ledger_keys = Vec::with_capacity(values.len().min(MAX_KEY_PATTERN_CANDIDATES));
        for key in values {
            let ledger_key = LedgerKey::ContractData(LedgerKeyContractData {
                contract: ScAddress::Contract(Hash(*contract_id)),
                key,
                durability: ContractDataDurability::Persistent,
            });
            let encoded = ledger_key
                .to_xdr_base64(safe_xdr_limits())
                .map_err(|error| {
                    SentinelError::SerializationError(format!(
                        "Failed to encode key pattern candidate: {error}"
                    ))
                })?;
            ledger_keys.push((encoded, ledger_key));
        }
        ledger_keys.sort_by(|a, b| a.0.cmp(&b.0));
        ledger_keys.dedup_by(|a, b| a.0 == b.0);
        ledger_keys.truncate(MAX_KEY_PATTERN_CANDIDATES);
        Ok(ledger_keys.into_iter().map(|(_, key)| key).collect())
    }

    fn expand_token(
        token: &KeyPatternToken,
        known: &KeyPatternCandidateInput,
    ) -> SentinelResult<Vec<ScVal>> {
        match token {
            KeyPatternToken::Literal(value) => Ok(vec![parse_key_expr(value)?]),
            KeyPatternToken::SymbolPrefix(prefix) => Self::expand_symbol_prefix(prefix, known),
            KeyPatternToken::AddressWildcard => Self::expand_addresses(known),
            KeyPatternToken::Composite(tokens) => {
                let mut options = Vec::with_capacity(tokens.len());
                for token in tokens {
                    options.push(Self::expand_token(token, known)?);
                }
                let combinations = Self::cartesian(options)?;
                let mut values = Vec::with_capacity(combinations.len());
                for combination in combinations {
                    let sc_vec: ScVec = combination.try_into().map_err(|error| {
                        SentinelError::InvalidXdrKey(format!(
                            "Failed to create composite key candidate: {error}"
                        ))
                    })?;
                    values.push(ScVal::Vec(Some(sc_vec)));
                }
                Self::sorted_unique(values)
            }
        }
    }

    fn expand_symbol_prefix(
        prefix: &str,
        known: &KeyPatternCandidateInput,
    ) -> SentinelResult<Vec<ScVal>> {
        parse_key_expr(&format!("sym:{prefix}"))?;
        let mut suffixes = known.symbol_suffixes.clone();
        suffixes.sort();
        suffixes.dedup();

        let mut values = Vec::new();
        for suffix in suffixes.into_iter().take(MAX_KEY_PATTERN_CANDIDATES) {
            let symbol = format!("{prefix}{suffix}");
            if let Ok(value) = parse_key_expr(&format!("sym:{symbol}")) {
                values.push(value);
            }
        }
        if values.is_empty() {
            values.push(parse_key_expr(&format!("sym:{prefix}"))?);
        }
        Self::sorted_unique(values)
    }

    fn expand_addresses(known: &KeyPatternCandidateInput) -> SentinelResult<Vec<ScVal>> {
        let mut addresses = known.addresses.clone();
        addresses.sort();
        addresses.dedup();

        let mut values = Vec::new();
        for address in addresses.into_iter().take(MAX_KEY_PATTERN_CANDIDATES) {
            values.push(parse_key_expr(&format!("addr:{address}"))?);
        }
        Self::sorted_unique(values)
    }

    fn cartesian(options: Vec<Vec<ScVal>>) -> SentinelResult<Vec<Vec<ScVal>>> {
        let mut combinations: Vec<Vec<ScVal>> = vec![Vec::new()];
        for option_set in options {
            if option_set.is_empty() {
                return Ok(Vec::new());
            }
            let capacity = combinations
                .len()
                .saturating_mul(option_set.len())
                .min(MAX_KEY_PATTERN_CANDIDATES);
            let mut next = Vec::with_capacity(capacity);
            'outer: for combination in &combinations {
                for option in &option_set {
                    if next.len() >= MAX_KEY_PATTERN_CANDIDATES {
                        break 'outer;
                    }
                    let mut extended = combination.clone();
                    extended.push(option.clone());
                    next.push(extended);
                }
            }
            combinations = next;
        }
        Ok(combinations)
    }

    fn sorted_unique(values: Vec<ScVal>) -> SentinelResult<Vec<ScVal>> {
        let mut encoded = Vec::with_capacity(values.len());
        for value in values {
            let xdr = value.to_xdr_base64(safe_xdr_limits()).map_err(|error| {
                SentinelError::SerializationError(format!(
                    "Failed to encode key pattern candidate: {error}"
                ))
            })?;
            encoded.push((xdr, value));
        }
        encoded.sort_by(|a, b| a.0.cmp(&b.0));
        encoded.dedup_by(|a, b| a.0 == b.0);
        Ok(encoded
            .into_iter()
            .take(MAX_KEY_PATTERN_CANDIDATES)
            .map(|(_, value)| value)
            .collect())
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
