//! Extract candidate storage keys from Soroban interface metadata.

use std::collections::BTreeSet;

use stellar_xdr::curr::{
    ContractCodeEntry, Limited, ReadXdr, ScSpecEntry, ScSpecUdtUnionCaseV0, ScSymbol, ScVal,
};
use wasmparser::{Parser, Payload};

use super::{safe_xdr_limits, XDR_MAX_BYTE_LENGTH};
use crate::{SentinelError, SentinelResult};

fn invalid(message: impl std::fmt::Display) -> SentinelError {
    SentinelError::SerializationError(format!("Invalid contract WASM spec: {message}"))
}

fn storage_type(name: &[u8]) -> bool {
    let name = String::from_utf8_lossy(name).to_ascii_lowercase();
    name.ends_with("datakey") || name.ends_with("storagekey") || name.ends_with("ledgerkey")
}

fn symbol(name: &[u8]) -> Option<ScSymbol> {
    if name.is_empty()
        || !name
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
    {
        return None;
    }
    name.to_vec().try_into().ok().map(ScSymbol)
}

/// Finds concrete key candidates in storage-named enum definitions.
/// Tuple cases carrying dynamic fields cannot be inferred from an interface.
pub fn storage_key_candidates(code: &ContractCodeEntry) -> SentinelResult<Vec<ScVal>> {
    let wasm = code.code.as_slice();
    if wasm.len() > XDR_MAX_BYTE_LENGTH {
        return Err(invalid("WASM exceeds the size limit"));
    }

    let mut keys = BTreeSet::new();
    for payload in Parser::new(0).parse_all(wasm) {
        let Payload::CustomSection(section) = payload.map_err(invalid)? else {
            continue;
        };
        if section.name() != "contractspecv0" {
            continue;
        }
        let mut input = Limited::new(section.data(), safe_xdr_limits());
        for entry in ScSpecEntry::read_xdr_iter(&mut input) {
            match entry.map_err(invalid)? {
                ScSpecEntry::UdtUnionV0(union) if storage_type(union.name.as_slice()) => {
                    for case in union.cases.iter() {
                        if let ScSpecUdtUnionCaseV0::VoidV0(case) = case {
                            if let Some(name) = symbol(case.name.as_slice()) {
                                let bare = ScVal::Symbol(name);
                                keys.insert(bare.clone());
                                keys.insert(ScVal::Vec(Some(stellar_xdr::curr::ScVec(
                                    vec![bare].try_into().map_err(invalid)?,
                                ))));
                            }
                        }
                    }
                }
                ScSpecEntry::UdtEnumV0(enumeration)
                    if storage_type(enumeration.name.as_slice()) =>
                {
                    keys.extend(enumeration.cases.iter().map(|case| ScVal::U32(case.value)));
                }
                _ => {}
            }
            if keys.len() > 256 {
                return Err(invalid("too many storage key candidates"));
            }
        }
    }
    Ok(keys.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use stellar_xdr::curr::{ContractCodeEntryExt, Hash};

    #[test]
    fn malformed_spec_fails_and_missing_spec_returns_no_candidates() {
        let mut entry = ContractCodeEntry {
            ext: ContractCodeEntryExt::V0,
            hash: Hash([0; 32]),
            code: b"\0asm\x01\0\0\0".to_vec().try_into().unwrap(),
        };
        assert!(storage_key_candidates(&entry).unwrap().is_empty());

        // Valid custom-section framing containing incomplete XDR.
        let mut wasm = entry.code.as_slice().to_vec();
        wasm.extend_from_slice(b"\0\x10\x0econtractspecv0\xff");
        entry.code = wasm.try_into().unwrap();
        assert!(storage_key_candidates(&entry).is_err());
    }
}
