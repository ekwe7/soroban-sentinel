use assert_cmd::Command;
use sentinel_core::derive_contract_instance_key;
use serde_json::Value;
use stellar_strkey::Contract;
use stellar_xdr::curr::{
    ContractDataDurability, ContractDataEntry, ContractExecutable, ExtensionPoint, Hash,
    LedgerEntryData, Limits, ScAddress, ScContractInstance, ScVal, WriteXdr,
};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn instance_xdr(contract: [u8; 32]) -> String {
    LedgerEntryData::ContractData(ContractDataEntry {
        ext: ExtensionPoint::V0,
        contract: ScAddress::Contract(Hash(contract)),
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
async fn scan_sends_discord_alert_for_warn_status() {
    let rpc = MockServer::start().await;
    let webhook = MockServer::start().await;
    let contract_bytes = [8; 32];
    let contract = Contract(contract_bytes).to_string();
    let key = derive_contract_instance_key(&contract_bytes).unwrap();
    let xdr = instance_xdr(contract_bytes);
    Mock::given(method("POST")).and(path("/")).respond_with(move |request: &wiremock::Request| {
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        match body["method"].as_str().unwrap() {
            "getLatestLedger" => ResponseTemplate::new(200).set_body_json(serde_json::json!({"jsonrpc":"2.0","id":1,"result":{"sequence":100,"protocolVersion":22}})),
            "getLedgerEntries" => ResponseTemplate::new(200).set_body_json(serde_json::json!({"jsonrpc":"2.0","id":1,"result":{"latestLedger":100,"entries":[{"key":key,"xdr":xdr,"liveUntilLedgerSeq":1000,"lastModifiedLedgerSeq":90}]}})),
            "getLedgers" => ResponseTemplate::new(200).set_body_json(serde_json::json!({"jsonrpc":"2.0","id":1,"result":{"ledgers":[]}})),
            method => panic!("unexpected method: {method}"),
        }
    }).mount(&rpc).await;
    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&webhook)
        .await;
    let output = Command::cargo_bin("sentinel")
        .unwrap()
        .args([
            "scan",
            &contract,
            "--rpc-url",
            &rpc.uri(),
            "--alert-webhook",
            &webhook.uri(),
            "--alert-kind",
            "discord",
        ])
        .assert()
        .code(2)
        .get_output()
        .stdout
        .clone();
    assert!(String::from_utf8_lossy(&output).contains("CRITICAL"));
    let requests = webhook.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    let body: Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert!(body["embeds"][0]["description"]
        .as_str()
        .unwrap()
        .contains(&contract));
}
