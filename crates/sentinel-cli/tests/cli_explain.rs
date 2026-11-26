use assert_cmd::Command;
use predicates::prelude::*;
use stellar_xdr::curr::{
    ContractCodeEntry, ContractCodeEntryExt, ContractDataDurability, ContractDataEntry,
    ContractExecutable, ExtensionPoint, Hash, LedgerEntry, LedgerEntryData, LedgerEntryExt,
    LedgerKey, LedgerKeyContractData, Limits, ScAddress, ScContractInstance, ScMap, ScMapEntry,
    ScSymbol, ScVal, WriteXdr,
};

const CONTRACT: [u8; 32] = [7; 32];
const WASM_HASH: [u8; 32] = [0xab; 32];

fn contract_strkey() -> String {
    stellar_strkey::Contract(CONTRACT).to_string()
}

fn symbol(name: &str) -> ScVal {
    ScVal::Symbol(ScSymbol(name.try_into().unwrap()))
}

fn instance_entry() -> LedgerEntry {
    let storage = ScMap(
        vec![
            ScMapEntry {
                key: symbol("ADMIN"),
                val: ScVal::U32(1),
            },
            ScMapEntry {
                key: symbol("PAUSED"),
                val: ScVal::Bool(false),
            },
        ]
        .try_into()
        .unwrap(),
    );
    LedgerEntry {
        last_modified_ledger_seq: 5_390_000,
        data: LedgerEntryData::ContractData(ContractDataEntry {
            ext: ExtensionPoint::V0,
            contract: ScAddress::Contract(Hash(CONTRACT)),
            key: ScVal::LedgerKeyContractInstance,
            durability: ContractDataDurability::Persistent,
            val: ScVal::ContractInstance(ScContractInstance {
                executable: ContractExecutable::Wasm(Hash(WASM_HASH)),
                storage: Some(storage),
            }),
        }),
        ext: LedgerEntryExt::V0,
    }
}

fn explain() -> Command {
    let mut command = Command::cargo_bin("sentinel").unwrap();
    command.arg("explain");
    command
}

fn json_output(command: &mut Command) -> serde_json::Value {
    let output = command.assert().success().get_output().stdout.clone();
    serde_json::from_slice(&output).unwrap()
}

#[test]
fn explain_ledger_entry_text_shows_instance_details() {
    let entry = instance_entry();
    let size = entry.to_xdr(Limits::none()).unwrap().len();

    explain()
        .arg(entry.to_xdr_base64(Limits::none()).unwrap())
        .assert()
        .success()
        .stdout(predicate::str::contains("XDR type:        LedgerEntry"))
        .stdout(predicate::str::contains("Entry kind:      ContractData"))
        .stdout(predicate::str::contains(format!(
            "Contract ID:     {}",
            contract_strkey()
        )))
        .stdout(predicate::str::contains("Durability:      persistent"))
        .stdout(predicate::str::contains(
            "Key:             LedgerKeyContractInstance",
        ))
        .stdout(predicate::str::contains(format!(
            "WASM hash:       {}",
            "ab".repeat(32)
        )))
        .stdout(predicate::str::contains("Instance keys:   2"))
        .stdout(predicate::str::contains("Last modified:   5390000"))
        .stdout(predicate::str::contains(format!(
            "XDR size:        {size} bytes"
        )));
}

#[test]
fn explain_ledger_entry_json_reports_structured_fields() {
    let entry = instance_entry();
    let json = json_output(
        explain()
            .arg(entry.to_xdr_base64(Limits::none()).unwrap())
            .args(["--format", "json"]),
    );

    assert_eq!(json["xdr_type"], "LedgerEntry");
    assert_eq!(json["contract_id"], contract_strkey());
    assert_eq!(json["durability"], "persistent");
    assert_eq!(json["executable"], "wasm");
    assert_eq!(json["wasm_hash"], "ab".repeat(32));
    assert_eq!(json["instance_storage_count"], 2);
    assert_eq!(json["last_modified_ledger"], 5_390_000);
    assert_eq!(
        json["xdr_byte_size"],
        entry.to_xdr(Limits::none()).unwrap().len()
    );
}

#[test]
fn explain_ledger_entry_data_for_contract_code() {
    let code = [0u8, 97, 115, 109, 1, 0, 0, 0];
    let data = LedgerEntryData::ContractCode(ContractCodeEntry {
        ext: ContractCodeEntryExt::V0,
        hash: Hash(WASM_HASH),
        code: code.to_vec().try_into().unwrap(),
    });

    let json = json_output(
        explain()
            .arg(data.to_xdr_base64(Limits::none()).unwrap())
            .args(["--format", "json"]),
    );

    assert_eq!(json["xdr_type"], "LedgerEntryData");
    assert_eq!(json["entry_kind"], "ContractCode");
    assert_eq!(json["wasm_hash"], "ab".repeat(32));
    assert_eq!(json["code_size"], code.len());
    assert!(json.get("contract_id").is_none());
    assert!(json.get("last_modified_ledger").is_none());
}

#[test]
fn explain_reads_ledger_key_from_stdin() {
    let key = LedgerKey::ContractData(LedgerKeyContractData {
        contract: ScAddress::Contract(Hash(CONTRACT)),
        key: symbol("Nonce"),
        durability: ContractDataDurability::Temporary,
    });
    let encoded = key.to_xdr_base64(Limits::none()).unwrap();
    // Wrapped base64 with a trailing newline, as produced by many tools.
    let (head, tail) = encoded.split_at(20);
    let piped = format!("{head}\n{tail}\n");

    let json = json_output(
        explain()
            .args(["--stdin", "--format", "json"])
            .write_stdin(piped),
    );

    assert_eq!(json["xdr_type"], "LedgerKey");
    assert_eq!(json["contract_id"], contract_strkey());
    assert_eq!(json["durability"], "temporary");
    assert_eq!(json["key"], "Nonce");
    assert_eq!(
        json["xdr_byte_size"],
        key.to_xdr(Limits::none()).unwrap().len()
    );
}

#[test]
fn explain_rejects_invalid_xdr_with_invalid_args_exit_code() {
    for input in ["not-base64!!", "AAAAAwAAAAc="] {
        explain()
            .arg(input)
            .assert()
            .code(10)
            .stderr(predicate::str::contains(
                "not a valid base64 LedgerEntry, LedgerEntryData, or LedgerKey XDR",
            ));
    }
}

#[test]
fn explain_rejects_empty_stdin() {
    explain()
        .arg("--stdin")
        .write_stdin("  \n")
        .assert()
        .code(10)
        .stderr(predicate::str::contains("no XDR input provided"));
}

#[test]
fn explain_requires_exactly_one_input_mode() {
    explain().assert().code(10);
    explain().args(["AAAA", "--stdin"]).assert().code(10);
}
