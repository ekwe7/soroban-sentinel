//! WASM binary parsing for Soroban ContractCode ledger entries.

use stellar_xdr::curr::{ContractCodeEntry, LedgerEntryData, ReadXdr};
use wasmparser::{Encoding, Parser, Payload, Validator};

use super::xdr_keys::safe_xdr_limits;
use crate::domain::code::{
    SorobanInterfaceVersion, WasmCodeInspection, WasmInspectionContext, WasmSectionSize,
    DEFAULT_MAX_WASM_SIZE_BYTES,
};
use crate::error::{SentinelError, SentinelResult};

const CONTRACT_SPEC_SECTION: &str = "contractspecv0";
const CONTRACT_ENV_META_SECTION: &str = "contractenvmetav0";
const CONTRACT_META_SECTION: &str = "contractmetav0";
const MAX_METADATA_ENTRIES: u32 = 256;

/// Parses and summarizes Soroban WASM bytecode.
pub fn inspect_wasm(
    wasm: &[u8],
    code_remaining_ledgers: i64,
    instance_remaining_ledgers: i64,
    critical_ledgers: u32,
) -> SentinelResult<WasmCodeInspection> {
    Validator::new()
        .validate_all(wasm)
        .map_err(|error| invalid_wasm(error.to_string()))?;

    let mut section_sizes = Vec::new();
    let mut contract_spec_size_bytes: Option<usize> = None;
    let mut interface_version = None;
    let mut sdk_version = None;
    let mut build_id = None;
    let mut env_meta_seen = false;
    let mut contract_meta_seen = false;

    for payload in Parser::new(0).parse_all(wasm) {
        let payload = payload.map_err(|error| invalid_wasm(error.to_string()))?;
        let section = match payload {
            Payload::Version { encoding, .. } if encoding != Encoding::Module => {
                return Err(invalid_wasm(
                    "expected a core WebAssembly module".to_string(),
                ));
            }
            Payload::TypeSection(reader) => Some((1, "type", reader.range())),
            Payload::ImportSection(reader) => Some((2, "import", reader.range())),
            Payload::FunctionSection(reader) => Some((3, "function", reader.range())),
            Payload::TableSection(reader) => Some((4, "table", reader.range())),
            Payload::MemorySection(reader) => Some((5, "memory", reader.range())),
            Payload::GlobalSection(reader) => Some((6, "global", reader.range())),
            Payload::ExportSection(reader) => Some((7, "export", reader.range())),
            Payload::StartSection { range, .. } => Some((8, "start", range)),
            Payload::ElementSection(reader) => Some((9, "element", reader.range())),
            Payload::CodeSectionStart { range, .. } => Some((10, "code", range)),
            Payload::DataSection(reader) => Some((11, "data", reader.range())),
            Payload::DataCountSection { range, .. } => Some((12, "data_count", range)),
            Payload::TagSection(reader) => Some((13, "tag", reader.range())),
            Payload::CustomSection(reader) => {
                let section_size = reader.range().len();
                let data = reader.data();
                match reader.name() {
                    CONTRACT_SPEC_SECTION => {
                        let total = contract_spec_size_bytes.unwrap_or(0);
                        contract_spec_size_bytes =
                            Some(total.checked_add(data.len()).ok_or_else(|| {
                                invalid_wasm("contract spec size overflow".to_string())
                            })?);
                    }
                    CONTRACT_ENV_META_SECTION => {
                        if env_meta_seen {
                            return Err(invalid_wasm(
                                "duplicate contractenvmetav0 section".to_string(),
                            ));
                        }
                        env_meta_seen = true;
                        let parsed = parse_interface_version(data)?;
                        interface_version = Some(parsed);
                    }
                    CONTRACT_META_SECTION => {
                        if contract_meta_seen {
                            return Err(invalid_wasm(
                                "duplicate contractmetav0 section".to_string(),
                            ));
                        }
                        contract_meta_seen = true;
                        let metadata = parse_contract_metadata(data)?;
                        sdk_version = metadata.0;
                        build_id = metadata.1;
                    }
                    _ => {}
                }
                section_sizes.push(WasmSectionSize {
                    id: 0,
                    name: format!("custom:{}", reader.name()),
                    size_bytes: section_size,
                });
                None
            }
            Payload::UnknownSection { id, range, .. } => Some((id, "unknown", range)),
            _ => None,
        };

        if let Some((id, name, range)) = section {
            section_sizes.push(WasmSectionSize {
                id,
                name: name.to_string(),
                size_bytes: range.len(),
            });
        }
    }

    Ok(WasmCodeInspection::new(
        wasm.len(),
        section_sizes,
        contract_spec_size_bytes,
        interface_version,
        sdk_version,
        build_id,
        WasmInspectionContext {
            code_remaining_ledgers,
            instance_remaining_ledgers,
            critical_ledgers,
            max_wasm_size_bytes: DEFAULT_MAX_WASM_SIZE_BYTES,
        },
    ))
}

/// Decodes a ContractCode entry from XDR and inspects its raw WASM payload.
pub fn inspect_contract_code_entry(
    entry_xdr_base64: &str,
    code_remaining_ledgers: i64,
    instance_remaining_ledgers: i64,
    critical_ledgers: u32,
) -> SentinelResult<WasmCodeInspection> {
    let entry = LedgerEntryData::from_xdr_base64(entry_xdr_base64, safe_xdr_limits())
        .map_err(|error| invalid_wasm(format!("invalid ContractCode XDR: {error}")))?;
    let ContractCodeEntry { code, .. } = match entry {
        LedgerEntryData::ContractCode(code) => code,
        _ => {
            return Err(invalid_wasm(
                "decoded ledger entry is not ContractCode".to_string(),
            ))
        }
    };
    inspect_wasm(
        code.as_slice(),
        code_remaining_ledgers,
        instance_remaining_ledgers,
        critical_ledgers,
    )
}

fn parse_interface_version(data: &[u8]) -> SentinelResult<SorobanInterfaceVersion> {
    let mut offset = 0;
    let count = read_u32(data, &mut offset)?;
    if count == 0 || count > MAX_METADATA_ENTRIES {
        return Err(invalid_wasm(
            "contractenvmetav0 has an invalid entry count".to_string(),
        ));
    }

    let mut version = None;
    for _ in 0..count {
        let kind = read_u32(data, &mut offset)?;
        if kind != 0 {
            return Err(invalid_wasm(format!(
                "unsupported contract environment metadata kind {kind}"
            )));
        }
        let parsed = SorobanInterfaceVersion {
            protocol: read_u32(data, &mut offset)?,
            pre_release: read_u32(data, &mut offset)?,
        };
        if version.replace(parsed).is_some() {
            return Err(invalid_wasm(
                "duplicate environment interface version".to_string(),
            ));
        }
    }
    ensure_xdr_end(data, offset)?;
    version.ok_or_else(|| invalid_wasm("missing environment interface version".to_string()))
}

fn parse_contract_metadata(data: &[u8]) -> SentinelResult<(Option<String>, Option<String>)> {
    let mut offset = 0;
    let count = read_u32(data, &mut offset)?;
    if count > MAX_METADATA_ENTRIES {
        return Err(invalid_wasm(
            "contractmetav0 has an invalid entry count".to_string(),
        ));
    }

    let mut sdk_version = None;
    let mut build_id = None;
    for _ in 0..count {
        let kind = read_u32(data, &mut offset)?;
        if kind != 0 {
            return Err(invalid_wasm(format!(
                "unsupported contract metadata kind {kind}"
            )));
        }
        let key = read_xdr_string(data, &mut offset)?;
        let value = read_xdr_string(data, &mut offset)?;
        match key.as_str() {
            "rssdkver" => sdk_version = Some(value),
            "build_id" | "build-id" | "buildId" => build_id = Some(value),
            _ => {}
        }
    }
    ensure_xdr_end(data, offset)?;
    Ok((sdk_version, build_id))
}

fn read_u32(data: &[u8], offset: &mut usize) -> SentinelResult<u32> {
    let end = offset
        .checked_add(4)
        .ok_or_else(|| invalid_wasm("metadata offset overflow".to_string()))?;
    let bytes = data
        .get(*offset..end)
        .ok_or_else(|| invalid_wasm("truncated metadata XDR".to_string()))?;
    *offset = end;
    Ok(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn read_xdr_string(data: &[u8], offset: &mut usize) -> SentinelResult<String> {
    let length = usize::try_from(read_u32(data, offset)?)
        .map_err(|_| invalid_wasm("metadata string length overflow".to_string()))?;
    let padded_length = length
        .checked_add(3)
        .map(|value| value & !3)
        .ok_or_else(|| invalid_wasm("metadata string length overflow".to_string()))?;
    let end = offset
        .checked_add(padded_length)
        .ok_or_else(|| invalid_wasm("metadata offset overflow".to_string()))?;
    let value_end = offset
        .checked_add(length)
        .ok_or_else(|| invalid_wasm("metadata offset overflow".to_string()))?;
    let value = data
        .get(*offset..value_end)
        .ok_or_else(|| invalid_wasm("truncated metadata string".to_string()))?;
    let padded = data
        .get(value_end..end)
        .ok_or_else(|| invalid_wasm("truncated metadata padding".to_string()))?;
    if padded.iter().any(|byte| *byte != 0) {
        return Err(invalid_wasm("invalid metadata XDR padding".to_string()));
    }
    *offset = end;
    String::from_utf8(value.to_vec())
        .map_err(|error| invalid_wasm(format!("metadata string is not UTF-8: {error}")))
}

fn ensure_xdr_end(data: &[u8], offset: usize) -> SentinelResult<()> {
    if offset == data.len() {
        Ok(())
    } else {
        Err(invalid_wasm(
            "unexpected trailing bytes in metadata XDR".to_string(),
        ))
    }
}

fn invalid_wasm(message: String) -> SentinelError {
    SentinelError::InvalidWasm(message)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn append_u32(out: &mut Vec<u8>, value: u32) {
        out.extend_from_slice(&value.to_be_bytes());
    }

    fn append_xdr_string(out: &mut Vec<u8>, value: &str) {
        append_u32(out, value.len() as u32);
        out.extend_from_slice(value.as_bytes());
        while out.len() % 4 != 0 {
            out.push(0);
        }
    }

    fn append_uleb(out: &mut Vec<u8>, mut value: usize) {
        loop {
            let mut byte = (value & 0x7f) as u8;
            value >>= 7;
            if value != 0 {
                byte |= 0x80;
            }
            out.push(byte);
            if value == 0 {
                break;
            }
        }
    }

    fn append_custom_section(wasm: &mut Vec<u8>, name: &str, data: &[u8]) {
        let mut payload = Vec::new();
        append_uleb(&mut payload, name.len());
        payload.extend_from_slice(name.as_bytes());
        payload.extend_from_slice(data);
        wasm.push(0);
        append_uleb(wasm, payload.len());
        wasm.extend_from_slice(&payload);
    }

    fn test_module() -> Vec<u8> {
        let mut wasm = b"\0asm\x01\0\0\0".to_vec();
        let mut environment = Vec::new();
        append_u32(&mut environment, 1);
        append_u32(&mut environment, 0);
        append_u32(&mut environment, 25);
        append_u32(&mut environment, 0);
        append_custom_section(&mut wasm, CONTRACT_ENV_META_SECTION, &environment);

        let mut metadata = Vec::new();
        append_u32(&mut metadata, 2);
        append_u32(&mut metadata, 0);
        append_xdr_string(&mut metadata, "rssdkver");
        append_xdr_string(&mut metadata, "25.3.1#abc");
        append_u32(&mut metadata, 0);
        append_xdr_string(&mut metadata, "build_id");
        append_xdr_string(&mut metadata, "release-42");
        append_custom_section(&mut wasm, CONTRACT_META_SECTION, &metadata);
        append_custom_section(&mut wasm, CONTRACT_SPEC_SECTION, &[1, 2, 3]);
        wasm
    }

    #[test]
    fn inspection_reads_soroban_sections_and_metadata() {
        let wasm = test_module();
        let report = inspect_wasm(&wasm, 100, 120, 34_560).expect("valid WASM");

        assert_eq!(report.wasm_size_bytes, wasm.len());
        assert_eq!(report.interface_version.unwrap().protocol, 25);
        assert_eq!(report.sdk_version.as_deref(), Some("25.3.1#abc"));
        assert_eq!(report.build_id.as_deref(), Some("release-42"));
        assert_eq!(report.contract_spec_size_bytes, Some(3));
        assert_eq!(report.byte_ledger_exposure, (wasm.len() as u128) * 100);
        assert_eq!(report.ttl_delta_from_instance_ledgers, -20);
        assert!(!report.nearing_archival);
        assert!(report
            .section_sizes
            .iter()
            .any(|section| { section.name == format!("custom:{CONTRACT_SPEC_SECTION}") }));
    }

    #[test]
    fn inspection_rejects_corrupt_wasm_and_metadata() {
        assert!(inspect_wasm(b"bad", 10, 10, 5).is_err());
        assert!(parse_interface_version(&[0, 0, 0, 1]).is_err());
    }
}
