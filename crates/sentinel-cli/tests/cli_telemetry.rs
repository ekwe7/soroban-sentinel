#![allow(unused_imports)]
//! CLI telemetry tests: an unreachable OTLP collector must never change scan output or exit code.

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
    let entry_data = LedgerEntryData::ContractData(ContractDataEntry {
        ext: ExtensionPoint::V0,
        contract: ScAddress::Contract(Hash(contract_id)),
        key: ScVal::LedgerKeyContractInstance,
        durability: ContractDataDurability::Persistent,
        val: ScVal::ContractInstance(ScContractInstance {
            executable: ContractExecutable::StellarAsset,
            storage: None,
        }),
    });
    entry_data.to_xdr_base64(Limits::none()).unwrap()
}

// Collector unreachable (--otlp-endpoint): scan must still succeed with normal output.
#[tokio::test]
async fn scan_succeeds_when_otlp_endpoint_flag_collector_is_unreachable() {
    let mock_server = MockServer::start().await;
    let contract_id = [1u8; 32];
    let contract_strkey = stellar_strkey::Contract(contract_id).to_string();
    let instance_key = derive_contract_instance_key(&contract_id).unwrap();
    let instance_xdr = make_instance_xdr(contract_id);

    // Live until 5,600,000 -> 200,000 ledgers remaining (> 120,960) -> OK
    let live_until = 5_600_000;

    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(move |req: &wiremock::Request| {
            let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
            match body["method"].as_str().unwrap() {
                "getLatestLedger" => ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "result": {
                        "sequence": 5_400_000,
                        "protocolVersion": 22
                    }
                })),
                "getLedgerEntries" => ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "result": {
                        "latestLedger": 5_400_000,
                        "entries": [{
                            "key": instance_key,
                            "xdr": instance_xdr,
                            "liveUntilLedgerSeq": live_until,
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
    cmd.arg("--otlp-endpoint").arg("http://127.0.0.1:1");
    cmd.args([
        "scan",
        &contract_strkey,
        "--rpc-url",
        &mock_server.uri(),
        "--format",
        "text",
    ]);

    cmd.assert()
        .code(0)
        .stdout(predicate::str::contains(&contract_strkey))
        .stdout(predicate::str::contains("Entry Description"))
        .stdout(predicate::str::contains("Live Until Ledger"))
        .stdout(predicate::str::contains("Contract Instance"))
        .stdout(predicate::str::contains(
            "Summary: 1 entries checked. 1 OK, 0 WARN, 0 CRITICAL/ARCHIVED.",
        ));
}

// Collector unreachable (OTEL_EXPORTER_OTLP_ENDPOINT env var): same guarantee.
#[tokio::test]
async fn scan_succeeds_when_otlp_env_collector_is_unreachable() {
    let mock_server = MockServer::start().await;
    let contract_id = [1u8; 32];
    let contract_strkey = stellar_strkey::Contract(contract_id).to_string();
    let instance_key = derive_contract_instance_key(&contract_id).unwrap();
    let instance_xdr = make_instance_xdr(contract_id);

    // Live until 5,600,000 -> 200,000 ledgers remaining (> 120,960) -> OK
    let live_until = 5_600_000;

    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(move |req: &wiremock::Request| {
            let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
            match body["method"].as_str().unwrap() {
                "getLatestLedger" => ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "result": {
                        "sequence": 5_400_000,
                        "protocolVersion": 22
                    }
                })),
                "getLedgerEntries" => ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "result": {
                        "latestLedger": 5_400_000,
                        "entries": [{
                            "key": instance_key,
                            "xdr": instance_xdr,
                            "liveUntilLedgerSeq": live_until,
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
    cmd.env("OTEL_EXPORTER_OTLP_ENDPOINT", "http://127.0.0.1:1");
    cmd.args([
        "scan",
        &contract_strkey,
        "--rpc-url",
        &mock_server.uri(),
        "--format",
        "text",
    ]);

    cmd.assert()
        .code(0)
        .stdout(predicate::str::contains(&contract_strkey))
        .stdout(predicate::str::contains("Entry Description"))
        .stdout(predicate::str::contains("Live Until Ledger"))
        .stdout(predicate::str::contains("Contract Instance"))
        .stdout(predicate::str::contains(
            "Summary: 1 entries checked. 1 OK, 0 WARN, 0 CRITICAL/ARCHIVED.",
        ));
}

#[test]
fn otlp_endpoint_flag_is_advertised_in_help() {
    Command::cargo_bin("sentinel")
        .unwrap()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicates::str::contains("--otlp-endpoint"));
}
