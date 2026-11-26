use sentinel_core::adapters::inspect_wasm;

const EMPTY_WASM_MODULE: &[u8] = b"\0asm\x01\0\0\0";

#[test]
fn inspects_valid_contract_wasm_and_ttl_exposure() {
    let inspection =
        inspect_wasm(EMPTY_WASM_MODULE, 10_000, 12_000, 34_560).expect("valid WASM module");

    assert_eq!(inspection.wasm_size_bytes, EMPTY_WASM_MODULE.len());
    assert_eq!(inspection.code_remaining_ledgers, 10_000);
    assert_eq!(inspection.instance_remaining_ledgers, 12_000);
    assert_eq!(inspection.ttl_delta_from_instance_ledgers, -2_000);
    assert_eq!(
        inspection.byte_ledger_exposure,
        EMPTY_WASM_MODULE.len() as u128 * 10_000
    );
    assert!(inspection.nearing_archival);
}

#[test]
fn rejects_corrupt_wasm() {
    assert!(inspect_wasm(b"not wasm", 0, 0, 34_560).is_err());
    assert!(inspect_wasm(b"\0asm\x02\0\0\0", 0, 0, 34_560).is_err());
}
