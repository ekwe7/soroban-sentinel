use assert_cmd::Command;
use predicates::prelude::*;
use sentinel_core::derive_contract_instance_key;
use stellar_xdr::curr::{
    ContractDataDurability, ContractDataEntry, ContractExecutable, ExtensionPoint, Hash,
    LedgerEntryData, LedgerKey, Limits, ReadXdr, ScAddress, ScContractInstance, ScVal, WriteXdr,
};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn instance_xdr(contract_id: [u8; 32]) -> String {
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
async fn estimate_gas_reports_json_and_table_using_mock_testnet_footprint() {
    let server = MockServer::start().await;
    let contract_id = [12u8; 32];
    let contract = stellar_strkey::Contract(contract_id).to_string();
    let key_xdr = derive_contract_instance_key(&contract_id).unwrap();
    let entry_xdr = instance_xdr(contract_id);
    let response_key_xdr = key_xdr.clone();
    let response_entry_xdr = entry_xdr.clone();
    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(move |request: &wiremock::Request| {
            let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
            assert_eq!(body["method"], "getLedgerEntries");
            ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "jsonrpc":"2.0", "id":1,
                "result":{"latestLedger":5_400_000,"entries":[{
                    "key":response_key_xdr,"xdr":response_entry_xdr,
                    "liveUntilLedgerSeq":5_600_000,"lastModifiedLedgerSeq":5_390_000
                }]}
            }))
        })
        .mount(&server)
        .await;

    let mut json_cmd = Command::cargo_bin("sentinel").unwrap();
    let json_output = json_cmd
        .args([
            "estimate-gas",
            &contract,
            "--extend-by",
            "5000",
            "--rpc-url",
            &server.uri(),
            "--format",
            "json",
        ])
        .output()
        .unwrap();
    assert_eq!(
        json_output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&json_output.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&json_output.stdout).unwrap();
    let key = LedgerKey::from_xdr_base64(&key_xdr, Limits::none()).unwrap();
    let data = LedgerEntryData::from_xdr_base64(&entry_xdr, Limits::none()).unwrap();
    let key_bytes = key.to_xdr(Limits::none()).unwrap().len() as u64;
    let entry_bytes = data.to_xdr(Limits::none()).unwrap().len() as u64;
    let read_bytes = key_bytes + entry_bytes;
    let estimated_cpu = 75_000 + 50_000 + entry_bytes * 100;
    assert_eq!(json["extend_by_ledgers"], 5000);
    assert_eq!(json["estimated_read_bytes"], read_bytes);
    assert_eq!(json["estimated_write_bytes"], entry_bytes);
    assert_eq!(json["estimated_cpu_instructions"], estimated_cpu);
    assert_eq!(
        json["recommended_cpu_instructions"],
        (estimated_cpu * 115).div_ceil(100)
    );
    assert_eq!(json["entries"][0]["entry_type"], "contract_instance");
    assert!(json["estimate_method"]
        .as_str()
        .unwrap()
        .contains("planning heuristic"));

    let mut text_cmd = Command::cargo_bin("sentinel").unwrap();
    text_cmd.args([
        "estimate-gas",
        &contract,
        "--extend-by",
        "5000",
        "--rpc-url",
        &server.uri(),
        "--format",
        "text",
    ]);
    text_cmd
        .assert()
        .success()
        .stdout(predicate::str::contains("Key XDR B"))
        .stdout(predicate::str::contains("Recommended with 15% buffer"))
        .stdout(predicate::str::contains("planning heuristic"));
}
