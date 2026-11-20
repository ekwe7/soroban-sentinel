use assert_cmd::Command;
use predicates::prelude::*;
use stellar_xdr::curr::{
    ContractDataDurability, ContractDataEntry, ContractExecutable, ExtensionPoint, Hash,
    LedgerEntryData, Limits, ScAddress, ScContractInstance, ScVal, WriteXdr,
};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use sentinel_core::adapters::derive_contract_instance_key;

fn make_instance_xdr(contract_id: [u8; 32]) -> String {
    LedgerEntryData::ContractData(ContractDataEntry {
        ext: ExtensionPoint::V0,
        contract: ScAddress::Contract(Hash(contract_id)),
        key: ScVal::LedgerKeyContractInstance,
        durability: ContractDataDurability::Persistent,
        val: ScVal::ContractInstance(ScContractInstance {
            executable: ContractExecutable::StellarAsset,
            storage: None,
        }),
    })
    .to_xdr_base64(Limits::none())
    .unwrap()
}

#[tokio::test]
async fn test_cli_scan_sarif_output_is_valid() {
    let mock_server = MockServer::start().await;
    let contract_id = [42u8; 32];
    let contract_strkey = stellar_strkey::Contract(contract_id).to_string();
    let instance_key = derive_contract_instance_key(&contract_id).unwrap();
    let instance_xdr = make_instance_xdr(contract_id);

    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(move |req: &wiremock::Request| {
            let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
            match body["method"].as_str().unwrap() {
                "getLatestLedger" => ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "jsonrpc": "2.0", "id": 1,
                    "result": { "sequence": 5_400_000, "protocolVersion": 22 }
                })),
                "getLedgerEntries" => ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "jsonrpc": "2.0", "id": 1,
                    "result": {
                        "latestLedger": 5_400_000,
                        "entries": [{
                            "key": instance_key,
                            "xdr": instance_xdr,
                            "liveUntilLedgerSeq": 5_500_000,
                            "lastModifiedLedgerSeq": 5_390_000
                        }]
                    }
                })),
                other => panic!("Unexpected method: {}", other),
            }
        })
        .mount(&mock_server)
        .await;

    let mut cmd = Command::cargo_bin("sentinel").unwrap();
    cmd.args([
        "scan",
        &contract_strkey,
        "--rpc-url",
        &mock_server.uri(),
        "--format",
        "sarif",
    ]);

    let output = cmd.output().unwrap();
    assert_eq!(output.status.code(), Some(1));

    let stdout = String::from_utf8(output.stdout).unwrap();
    let sarif: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(sarif["version"], "2.1.0");
    assert_eq!(
        sarif["runs"][0]["tool"]["driver"]["name"],
        "soroban-sentinel"
    );
    assert_eq!(sarif["runs"][0]["results"][0]["level"], "warning");
    assert!(sarif["runs"][0]["tool"]["driver"]["rules"].is_array());
    assert!(stdout.contains(&contract_strkey));
}

#[test]
fn sarif_format_is_advertised_in_help() {
    let mut cmd = Command::cargo_bin("sentinel").unwrap();
    cmd.args(["scan", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("sarif"));
}
