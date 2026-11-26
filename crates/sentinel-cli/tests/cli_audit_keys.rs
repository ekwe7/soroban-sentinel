use assert_cmd::Command;
use predicates::prelude::*;
use stellar_xdr::curr::{
    ContractDataDurability, ContractDataEntry, ContractExecutable, ExtensionPoint, Hash,
    LedgerEntryData, Limits, ScAddress, ScContractInstance, ScVal, WriteXdr,
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
async fn audit_keys_text_report_with_small_persistent_entry() {
    let mock = MockServer::start().await;
    let contract_id = [7u8; 32];
    let contract = stellar_strkey::Contract(contract_id).to_string();
    let instance_key = sentinel_core::derive_contract_instance_key(&contract_id).unwrap();

    Mock::given(method("POST")).and(path("/")).respond_with(move |req: &wiremock::Request| {
        let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
        match body["method"].as_str().unwrap() {
            "getLatestLedger" => ResponseTemplate::new(200).set_body_json(serde_json::json!({"jsonrpc":"2.0","id":1,"result":{"sequence":5_400_000,"protocolVersion":22}})),
            "getLedgerEntries" => ResponseTemplate::new(200).set_body_json(serde_json::json!({"jsonrpc":"2.0","id":1,"result":{"latestLedger":5_400_000,"entries":[{"key":instance_key,"xdr":instance_xdr(contract_id),"liveUntilLedgerSeq":5_410_000,"lastModifiedLedgerSeq":5_390_000}]}})),
            _ => ResponseTemplate::new(200).set_body_json(serde_json::json!({"jsonrpc":"2.0","id":1,"result":{"entries":[]}})),
        }
    }).mount(&mock).await;

    Command::cargo_bin("sentinel")
        .unwrap()
        .args([
            "audit-keys",
            &contract,
            "--rpc-url",
            &mock.uri(),
            "--format",
            "text",
        ])
        .assert()
        .code(0)
        .stdout(predicate::str::contains("Storage Audit:"))
        .stdout(predicate::str::contains("instance"));
}

#[tokio::test]
async fn audit_keys_json_report_is_machine_readable() {
    let mock = MockServer::start().await;
    let contract_id = [8u8; 32];
    let contract = stellar_strkey::Contract(contract_id).to_string();
    let instance_key = sentinel_core::derive_contract_instance_key(&contract_id).unwrap();

    Mock::given(method("POST")).and(path("/")).respond_with(move |req: &wiremock::Request| {
        let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
        match body["method"].as_str().unwrap() {
            "getLatestLedger" => ResponseTemplate::new(200).set_body_json(serde_json::json!({"jsonrpc":"2.0","id":1,"result":{"sequence":5_400_000,"protocolVersion":22}})),
            "getLedgerEntries" => ResponseTemplate::new(200).set_body_json(serde_json::json!({"jsonrpc":"2.0","id":1,"result":{"latestLedger":5_400_000,"entries":[{"key":instance_key,"xdr":instance_xdr(contract_id),"liveUntilLedgerSeq":5_600_000,"lastModifiedLedgerSeq":5_390_000}]}})),
            _ => ResponseTemplate::new(200).set_body_json(serde_json::json!({"jsonrpc":"2.0","id":1,"result":{"entries":[]}})),
        }
    }).mount(&mock).await;

    let output = Command::cargo_bin("sentinel")
        .unwrap()
        .args([
            "audit-keys",
            &contract,
            "--rpc-url",
            &mock.uri(),
            "--format",
            "json",
        ])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(0));
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["contract_id"], contract);
    assert_eq!(report["total_entries"], 1);
    assert!(report["entries"][0]["durability"].as_str().is_some());
}
