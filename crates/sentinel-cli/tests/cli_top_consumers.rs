use assert_cmd::Command;
use predicates::prelude::*;
use sentinel_core::{adapters::derive_contract_instance_key, derive_persistent_symbol_key};
use serde_json::{json, Value};
use stellar_xdr::curr::{
    ContractDataDurability, ContractDataEntry, ContractExecutable, ExtensionPoint, Hash,
    LedgerEntryData, Limits, ScAddress, ScContractInstance, ScVal, WriteXdr,
};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

fn id() -> String {
    stellar_strkey::Contract([7; 32]).to_string()
}

async fn mock_entries() -> (MockServer, usize) {
    let server = MockServer::start().await;
    let instance = ScVal::ContractInstance(ScContractInstance {
        executable: ContractExecutable::StellarAsset,
        storage: None,
    });
    let values = [
        (
            derive_contract_instance_key(&[7; 32]).unwrap(),
            ScVal::LedgerKeyContractInstance,
            instance,
        ),
        (
            derive_persistent_symbol_key(&[7; 32], "BIG").unwrap(),
            ScVal::Symbol("BIG".try_into().unwrap()),
            ScVal::Bytes(vec![0xaa; 1000].try_into().unwrap()),
        ),
        (
            derive_persistent_symbol_key(&[7; 32], "SMALL").unwrap(),
            ScVal::Symbol("SMALL".try_into().unwrap()),
            ScVal::U32(1),
        ),
    ];
    let mut total = 0;
    let entries: Vec<Value> = values
        .into_iter()
        .map(|(key, data_key, val)| {
            let data = LedgerEntryData::ContractData(ContractDataEntry {
                ext: ExtensionPoint::V0,
                contract: ScAddress::Contract(Hash([7; 32])),
                key: data_key,
                durability: ContractDataDurability::Persistent,
                val,
            });
            total += data.to_xdr(Limits::none()).unwrap().len();
            json!({"key": key, "xdr": data.to_xdr_base64(Limits::none()).unwrap(),
            "liveUntilLedgerSeq": 200000, "lastModifiedLedgerSeq": 900})
        })
        .collect();
    Mock::given(method("POST")).respond_with(move |request: &wiremock::Request| {
        let call: Value = serde_json::from_slice(&request.body).unwrap();
        let result = match call["method"].as_str().unwrap() {
            "getLatestLedger" => json!({"sequence":1000, "protocolVersion":22}),
            "getLedgers" => json!({"ledgers":[]}),
            "getLedgerEntries" => {
                let keys = call["params"]["keys"].as_array().unwrap();
                json!({"entries": entries.iter().filter(|entry| keys.contains(&entry["key"])).collect::<Vec<_>>(), "latestLedger":1000})
            }
            _ => panic!("unexpected RPC method"),
        };
        ResponseTemplate::new(200).set_body_json(json!({"jsonrpc":"2.0","id":call["id"],"result": result}))
    }).mount(&server).await;
    (server, total)
}

fn command(server: &MockServer) -> Command {
    let mut cmd = Command::cargo_bin("sentinel").unwrap();
    cmd.args([
        "top-consumers",
        &id(),
        "--rpc-url",
        &server.uri(),
        "--key",
        "BIG",
        "--key",
        "SMALL",
    ]);
    cmd
}

#[tokio::test]
async fn json_sorts_raw_xdr_bytes_and_percentages_use_all_resolved_entries() {
    let (server, total) = mock_entries().await;
    let output = command(&server)
        .args(["--format", "json", "--limit", "1"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let report: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(report["total_entries"], 3);
    assert_eq!(report["total_bytes"], total);
    assert_eq!(report["entries"].as_array().unwrap().len(), 1);
    let largest = &report["entries"][0];
    assert!(largest["key_identifier"].as_str().unwrap().contains("BIG"));
    let expected = largest["byte_size"].as_f64().unwrap() / total as f64 * 100.0;
    assert!((largest["storage_percent"].as_f64().unwrap() - expected).abs() < 1e-10);
}

#[tokio::test]
async fn default_limit_text_and_zero_limit_work() {
    let (server, _) = mock_entries().await;
    command(&server)
        .assert()
        .success()
        .stdout(predicate::str::contains("XDR bytes"))
        .stdout(predicate::str::contains("%"))
        .stdout(predicate::str::contains("[#"));
    let output = command(&server)
        .args(["--format", "json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let report: Value = serde_json::from_slice(&output).unwrap();
    let entries = report["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 3);
    assert!(entries
        .windows(2)
        .all(|pair| pair[0]["byte_size"].as_u64() >= pair[1]["byte_size"].as_u64()));
    command(&server)
        .args(["--format", "json", "--limit", "0"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"entries\": []"));
}

#[test]
fn invalid_arguments_use_existing_cli_error_convention() {
    Command::cargo_bin("sentinel")
        .unwrap()
        .args(["top-consumers", "invalid"])
        .assert()
        .code(10);
    Command::cargo_bin("sentinel")
        .unwrap()
        .args(["top-consumers", &id(), "--limit", "-1"])
        .assert()
        .code(10);
}

#[test]
fn default_limit_is_ten() {
    use clap::Parser;
    use sentinel_cli::args::{Cli, Commands};
    let parsed = Cli::try_parse_from(["sentinel", "top-consumers", &id()]).unwrap();
    let Commands::TopConsumers(args) = parsed.command else {
        panic!("expected top-consumers")
    };
    assert_eq!(args.limit, 10);
}
