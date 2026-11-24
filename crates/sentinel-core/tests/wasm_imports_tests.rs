//! Tests for the WASM host-function import scanner (`scan_wasm_imports`).
//!
//! Fixtures are valid WASM modules assembled byte-by-byte with the same shape
//! as compiled Soroban contracts: `i64`-only signatures importing from the
//! single-letter host modules (`l`, `x`, `c`, `a`, ...).

use sentinel_core::{
    scan_wasm_imports, HostSubsystem, SentinelError, WasmImportKind, WasmValueType,
};

const I32: u8 = 0x7f;
const I64: u8 = 0x7e;

fn leb(mut value: u32, out: &mut Vec<u8>) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

fn name(text: &str) -> Vec<u8> {
    let mut out = Vec::new();
    leb(text.len() as u32, &mut out);
    out.extend_from_slice(text.as_bytes());
    out
}

fn section(id: u8, payload: Vec<u8>) -> Vec<u8> {
    let mut out = vec![id];
    leb(payload.len() as u32, &mut out);
    out.extend(payload);
    out
}

fn header() -> Vec<u8> {
    let mut out = b"\0asm".to_vec();
    out.extend_from_slice(&[1, 0, 0, 0]);
    out
}

fn type_section(signatures: &[(&[u8], &[u8])]) -> Vec<u8> {
    let mut payload = Vec::new();
    leb(signatures.len() as u32, &mut payload);
    for (params, results) in signatures {
        payload.push(0x60);
        leb(params.len() as u32, &mut payload);
        payload.extend_from_slice(params);
        leb(results.len() as u32, &mut payload);
        payload.extend_from_slice(results);
    }
    section(1, payload)
}

fn func_import(module: &str, field: &str, type_index: u32) -> Vec<u8> {
    let mut out = name(module);
    out.extend(name(field));
    out.push(0x00);
    leb(type_index, &mut out);
    out
}

fn memory_import(module: &str, field: &str) -> Vec<u8> {
    let mut out = name(module);
    out.extend(name(field));
    out.extend_from_slice(&[0x02, 0x00, 0x01]);
    out
}

fn global_import(module: &str, field: &str) -> Vec<u8> {
    let mut out = name(module);
    out.extend(name(field));
    out.extend_from_slice(&[0x03, I32, 0x00]);
    out
}

fn import_section(entries: Vec<Vec<u8>>) -> Vec<u8> {
    let mut payload = Vec::new();
    leb(entries.len() as u32, &mut payload);
    for entry in entries {
        payload.extend(entry);
    }
    section(2, payload)
}

fn module(signatures: &[(&[u8], &[u8])], imports: Vec<Vec<u8>>) -> Vec<u8> {
    let mut out = header();
    out.extend(type_section(signatures));
    out.extend(import_section(imports));
    out
}

fn expect_wasm_error(bytes: &[u8]) -> String {
    match scan_wasm_imports(bytes) {
        Err(SentinelError::WasmAnalysisError(message)) => message,
        other => panic!("expected WasmAnalysisError, got {other:?}"),
    }
}

#[test]
fn header_only_module_has_no_imports() {
    let profile = scan_wasm_imports(&header()).expect("valid empty module");
    assert_eq!(profile.total_imports(), 0);
    assert!(profile.module_counts.is_empty());
    assert!(profile.subsystem_counts.is_empty());
    assert!(profile.unrecognized_modules.is_empty());
    assert!(!profile.has_deprecated_imports());
}

#[test]
fn extracts_module_field_and_signature() {
    let bytes = module(
        &[(&[I64, I64, I64], &[I64]), (&[], &[I64])],
        vec![func_import("l", "0", 0), func_import("x", "_", 1)],
    );

    let profile = scan_wasm_imports(&bytes).expect("scan");
    assert_eq!(profile.total_imports(), 2);

    let first = &profile.imports[0];
    assert_eq!(first.module, "l");
    assert_eq!(first.field, "0");
    assert_eq!(first.kind, WasmImportKind::Function);
    let signature = first.signature.as_ref().expect("function signature");
    assert_eq!(signature.params, vec![WasmValueType::I64; 3]);
    assert_eq!(signature.results, vec![WasmValueType::I64]);

    let second = &profile.imports[1];
    assert_eq!(second.module, "x");
    assert_eq!(second.field, "_");
    let signature = second.signature.as_ref().expect("function signature");
    assert!(signature.params.is_empty());
    assert_eq!(signature.results, vec![WasmValueType::I64]);
}

#[test]
fn classifies_the_four_standard_subsystems() {
    let bytes = module(
        &[(&[I64], &[I64])],
        vec![
            func_import("l", "0", 0),
            func_import("l", "1", 0),
            func_import("x", "0", 0),
            func_import("c", "0", 0),
            func_import("a", "0", 0),
        ],
    );

    let profile = scan_wasm_imports(&bytes).expect("scan");
    assert_eq!(
        profile.subsystem_counts.get(&HostSubsystem::Ledger),
        Some(&2)
    );
    assert_eq!(
        profile.subsystem_counts.get(&HostSubsystem::Context),
        Some(&1)
    );
    assert_eq!(
        profile.subsystem_counts.get(&HostSubsystem::Crypto),
        Some(&1)
    );
    assert_eq!(profile.subsystem_counts.get(&HostSubsystem::Auth), Some(&1));
    assert_eq!(profile.module_counts.get("l"), Some(&2));
    assert_eq!(profile.imports_for(HostSubsystem::Ledger).count(), 2);
    assert_eq!(profile.imports_for(HostSubsystem::Auth).count(), 1);
    assert!(profile.unrecognized_modules.is_empty());
}

#[test]
fn other_host_modules_are_known_and_unknown_modules_are_reported() {
    let bytes = module(
        &[(&[I64], &[I64])],
        vec![
            func_import("v", "0", 0),
            func_import("env", "abort", 0),
            func_import("env", "memcpy", 0),
            func_import("zz", "0", 0),
        ],
    );

    let profile = scan_wasm_imports(&bytes).expect("scan");
    assert_eq!(
        profile.subsystem_counts.get(&HostSubsystem::Other),
        Some(&1)
    );
    assert_eq!(
        profile.subsystem_counts.get(&HostSubsystem::Unrecognized),
        Some(&3)
    );
    assert_eq!(
        profile.unrecognized_modules,
        vec!["env".to_string(), "zz".to_string()]
    );
}

#[test]
fn flags_retired_host_functions_only() {
    let bytes = module(
        &[(&[], &[I64])],
        vec![func_import("l", "0", 0), func_import("t", "0", 0)],
    );

    let profile = scan_wasm_imports(&bytes).expect("scan");
    assert!(profile.has_deprecated_imports());

    let deprecated: Vec<_> = profile.deprecated_imports().collect();
    assert_eq!(deprecated.len(), 1);
    assert_eq!(deprecated[0].module, "t");
    assert_eq!(deprecated[0].field, "0");
    assert_eq!(deprecated[0].max_supported_protocol, Some(19));
    assert!(!profile.imports[0].is_deprecated());
}

#[test]
fn non_function_imports_have_no_signature_or_deprecation() {
    let bytes = module(
        &[(&[], &[])],
        vec![memory_import("t", "0"), global_import("env", "g")],
    );

    let profile = scan_wasm_imports(&bytes).expect("scan");
    assert_eq!(profile.imports[0].kind, WasmImportKind::Memory);
    assert_eq!(profile.imports[1].kind, WasmImportKind::Global);
    assert!(profile
        .imports
        .iter()
        .all(|import| import.signature.is_none()));
    assert!(!profile.has_deprecated_imports());
}

#[test]
fn preserves_section_order_and_ignores_unrelated_sections() {
    let mut bytes = module(
        &[(&[I64], &[I64])],
        vec![
            func_import("c", "0", 0),
            func_import("a", "0", 0),
            func_import("l", "0", 0),
        ],
    );
    // Trailing custom section (id 0) named "n" with a small body.
    let mut custom = name("n");
    custom.extend_from_slice(&[1, 2, 3]);
    bytes.extend(section(0, custom));

    let profile = scan_wasm_imports(&bytes).expect("scan");
    let order: Vec<&str> = profile
        .imports
        .iter()
        .map(|import| import.module.as_str())
        .collect();
    assert_eq!(order, vec!["c", "a", "l"]);
}

#[test]
fn rejects_bad_magic_bytes() {
    let mut bytes = module(&[(&[], &[])], vec![]);
    bytes[0..4].copy_from_slice(b"\x7fELF");
    assert!(expect_wasm_error(&bytes).contains("magic"));
}

#[test]
fn rejects_truncated_header() {
    assert!(expect_wasm_error(&[]).contains("truncated"));
    assert!(expect_wasm_error(b"\0asm").contains("truncated"));
    assert!(expect_wasm_error(&b"\0asm\x01\x00\x00"[..]).contains("truncated"));
}

#[test]
fn rejects_unsupported_version() {
    let mut bytes = header();
    bytes[4] = 2;
    assert!(expect_wasm_error(&bytes).contains("version"));
}

#[test]
fn rejects_truncated_import_section() {
    let mut bytes = module(&[(&[I64], &[I64])], vec![func_import("l", "0", 0)]);
    bytes.pop();
    assert!(expect_wasm_error(&bytes).contains("malformed"));
}

#[test]
fn rejects_import_with_missing_type_index() {
    let bytes = module(&[(&[], &[])], vec![func_import("l", "0", 7)]);
    assert!(expect_wasm_error(&bytes).contains("type index 7"));
}
