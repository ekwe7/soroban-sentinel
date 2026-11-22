use assert_cmd::Command;
use predicates::prelude::*;
use sentinel_core::derive_contract_instance_key;
use serde_json::Value;
use stellar_strkey::ed25519::PublicKey as AccountStrkey;
use stellar_strkey::Contract;
use stellar_xdr::curr::{
    ContractDataDurability, ContractDataEntry, ContractExecutable, ExtensionPoint, Hash,
    LedgerEntryData, Limits, OperationBody, ReadXdr, ScAddress, ScContractInstance, ScVal,
    TransactionEnvelope, TransactionExt, WriteXdr,
};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn account() -> String {
    AccountStrkey([7; 32]).to_string()
}

fn instance_entry(contract: [u8; 32]) -> String {
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
async fn forecast_simulates_selected_footprint_and_reports_xlm() {
    let server = MockServer::start().await;
    let contract_bytes = [9; 32];
    let contract = Contract(contract_bytes).to_string();
    let instance_key = derive_contract_instance_key(&contract_bytes).unwrap();
    Mock::given(method("POST")).and(path("/")).respond_with(move |request: &wiremock::Request| {
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        match body["method"].as_str().unwrap() {
            "getLatestLedger" => ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "jsonrpc": "2.0", "id": 1, "result": {"sequence": 100, "protocolVersion": 22}
            })),
            "getLedgerEntries" => ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "jsonrpc": "2.0", "id": 1, "result": {"latestLedger": 100, "entries": [{
                    "key": instance_key, "xdr": instance_entry(contract_bytes), "liveUntilLedgerSeq": 500, "lastModifiedLedgerSeq": 90
                }]}
            })),
            "simulateTransaction" => {
                let xdr = body["params"]["transaction"].as_str().unwrap();
                let envelope = TransactionEnvelope::from_xdr_base64(xdr, Limits::none()).unwrap();
                let TransactionEnvelope::Tx(envelope) = envelope else { panic!("expected envelope") };
                assert_eq!(envelope.tx.operations.len(), 1);
                assert!(matches!(envelope.tx.operations[0].body, OperationBody::ExtendFootprintTtl(_)));
                let TransactionExt::V1(data) = envelope.tx.ext else { panic!("expected soroban data") };
                assert_eq!(data.resources.footprint.read_write.len(), 0);
                assert_eq!(data.resources.footprint.read_only.len(), 2);
                ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "jsonrpc": "2.0", "id": 1, "result": {"minResourceFee": "12345678", "latestLedger": 100}
                }))
            }
            method => panic!("unexpected method: {method}"),
        }
    }).mount(&server).await;
    let output = Command::cargo_bin("sentinel")
        .unwrap()
        .args([
            "forecast",
            &contract,
            "--extend-to",
            "1000",
            "--source",
            &account(),
            "--rpc-url",
            &server.uri(),
            "--key",
            "ADMIN",
            "--format",
            "json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(json["min_resource_fee_stroops"], 12345678);
    assert_eq!(json["min_resource_fee_xlm"], "1.2345678");
    assert_eq!(json["covered_entries"].as_array().unwrap().len(), 2);
}

#[test]
fn forecast_rejects_invalid_source_without_rpc() {
    Command::cargo_bin("sentinel")
        .unwrap()
        .args([
            "forecast",
            "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM",
            "--extend-to",
            "1000",
            "--source",
            "not-an-account",
        ])
        .assert()
        .code(10)
        .stderr(predicate::str::contains("invalid source account"));
}
