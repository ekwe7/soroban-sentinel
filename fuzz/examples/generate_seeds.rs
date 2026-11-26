//! Regenerates the small seed corpora from the same valid fixtures used by
//! sentinel-core tests. Run with `cargo run --manifest-path fuzz/Cargo.toml --example generate_seeds`.

use std::fs;
use std::path::{Path, PathBuf};

use stellar_xdr::curr::{
    ContractDataDurability, ContractDataEntry, ContractExecutable, ExtensionPoint, Hash,
    LedgerEntryData, Limits, ScAddress, ScContractInstance, ScSymbol, ScVal, WriteXdr,
};

fn write_seed(corpus: &str, name: &str, contents: &str) {
    let path: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("corpus")
        .join(corpus)
        .join(name);
    fs::create_dir_all(path.parent().expect("seed path has a parent")).expect("create corpus dir");
    fs::write(path, contents).expect("write seed");
}

fn main() {
    let contract_id = [9u8; 32];
    let wasm_hash = [7u8; 32];

    let instance = LedgerEntryData::ContractData(ContractDataEntry {
        ext: ExtensionPoint::V0,
        contract: ScAddress::Contract(Hash(contract_id)),
        key: ScVal::LedgerKeyContractInstance,
        durability: ContractDataDurability::Persistent,
        val: ScVal::ContractInstance(ScContractInstance {
            executable: ContractExecutable::Wasm(Hash(wasm_hash)),
            storage: None,
        }),
    })
    .to_xdr_base64(Limits::none())
    .expect("encode instance fixture");
    write_seed("extract_wasm_hash_from_instance_entry", "wasm_instance", &instance);

    let symbol_scval = ScVal::Symbol(ScSymbol("COUNTER".try_into().expect("valid symbol")));
    let symbol_xdr = symbol_scval
        .to_xdr_base64(Limits::none())
        .expect("encode symbol fixture");
    write_seed("derive_persistent_xdr_key", "symbol_scval", &symbol_xdr);

    let persistent = LedgerEntryData::ContractData(ContractDataEntry {
        ext: ExtensionPoint::V0,
        contract: ScAddress::Contract(Hash(contract_id)),
        key: symbol_scval,
        durability: ContractDataDurability::Persistent,
        val: ScVal::U32(42),
    })
    .to_xdr_base64(Limits::none())
    .expect("encode persistent fixture");
    write_seed("compute_byte_size", "persistent_entry", &persistent);

    let latest = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "result": { "id": "abc", "protocolVersion": 22, "sequence": 5_678_900 }
    })
    .to_string();
    write_seed("get_latest_ledger_response", "valid_response", &latest);

    let entries = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "result": {
            "latestLedger": 5_678_900,
            "entries": [{
                "key": "KEY_BASE64",
                "xdr": persistent,
                "liveUntilLedgerSeq": 5_700_000,
                "lastModifiedLedgerSeq": 5_650_000
            }]
        }
    })
    .to_string();
    write_seed("get_ledger_entries_response", "valid_response", &entries);
}
