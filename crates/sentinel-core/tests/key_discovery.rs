use sentinel_core::discover_storage_keys;
use stellar_xdr::curr::{
    ContractCodeEntry, ContractCodeEntryExt, ContractDataDurability, Hash, LedgerKey, Limits,
    ScSpecEntry, ScSpecUdtUnionCaseV0, ScSpecUdtUnionCaseVoidV0, ScSpecUdtUnionV0, ScVal, WriteXdr,
};

fn type_entry(name: &str, case: &str) -> ScSpecEntry {
    ScSpecEntry::UdtUnionV0(ScSpecUdtUnionV0 {
        name: name.try_into().unwrap(),
        doc: "".try_into().unwrap(),
        lib: "".try_into().unwrap(),
        cases: vec![ScSpecUdtUnionCaseV0::VoidV0(ScSpecUdtUnionCaseVoidV0 {
            name: case.try_into().unwrap(),
            doc: "".try_into().unwrap(),
        })]
        .try_into()
        .unwrap(),
    })
}

#[test]
fn key_discovery_reads_a_sample_wasm_spec_and_ignores_non_storage_types() {
    // A minimal WASM module with the same XDR custom section shape emitted by Soroban.
    let mut wasm = b"\0asm\x01\0\0\0".to_vec();
    let mut contents = b"\x0econtractspecv0".to_vec();
    for entry in [type_entry("DataKey", "Count"), type_entry("Order", "Paid")] {
        contents.extend(entry.to_xdr(Limits::none()).unwrap());
    }
    assert!(contents.len() < 128);
    wasm.extend([0, contents.len() as u8]);
    wasm.extend(contents);
    let code = ContractCodeEntry {
        ext: ContractCodeEntryExt::V0,
        hash: Hash([0; 32]),
        code: wasm.try_into().unwrap(),
    };
    let keys = discover_storage_keys(
        "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM",
        &code,
    )
    .unwrap();
    let symbol = ScVal::Symbol(stellar_xdr::curr::ScSymbol("Count".try_into().unwrap()));
    assert_eq!(keys.len(), 4);
    for durability in [
        ContractDataDurability::Persistent,
        ContractDataDurability::Temporary,
    ] {
        assert!(keys.iter().any(|key| matches!(key, LedgerKey::ContractData(data) if data.durability == durability && data.key == symbol)));
        assert!(keys.iter().any(
            |key| matches!(key, LedgerKey::ContractData(data) if data.durability == durability &&
            matches!(&data.key, ScVal::Vec(Some(list)) if list.as_slice() == std::slice::from_ref(&symbol)))
        ));
    }
}
