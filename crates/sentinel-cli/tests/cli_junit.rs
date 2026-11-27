use assert_cmd::Command;
use predicates::prelude::*;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use sentinel_core::adapters::derive_contract_instance_key;
use stellar_xdr::curr::{ContractDataDurability, ContractDataEntry, ContractExecutable,
    ExtensionPoint, Hash, LedgerEntryData, Limits, ScAddress, ScContractInstance, ScVal, WriteXdr};

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

async fn mock_server_with_instance_live_until(mock: &MockServer, live_until: u32, latest_ledger: u32) {
    let instance_key = derive_contract_instance_key(&[0u8; 32]).unwrap();
    let instance_xdr = make_instance_xdr([0u8; 32]);

    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(move |req: &wiremock::Request| {
            let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
            match body["method"].as_str().unwrap() {
                "getLatestLedger" => ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "result": {
                        "sequence": latest_ledger,
                        "protocolVersion": 22
                    }
                })),
                "getLedgerEntries" => ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "result": {
                        "latestLedger": latest_ledger,
                        "entries": [{
                            "key": instance_key,
                            "xdr": instance_xdr,
                            "liveUntilLedgerSeq": live_until,
                            "lastModifiedLedgerSeq": latest_ledger - 100_000
                        }]
                    }
                })),
                "getLedgers" => ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "result": {
                        "ledgers": [{
                            "sequence": latest_ledger,
                            "ledgerCloseTime": 1_700_000_000_000i64
                        }, {
                            "sequence": latest_ledger - 1,
                            "ledgerCloseTime": 1_700_000_000_000i64 - 5
                        }]
                    }
                })),
                other => panic!("Unexpected method: {}", other),
            }
        })
        .mount(mock)
        .await;
}

#[tokio::test]
async fn junit_format_outputs_valid_junit_xml_for_ok_entry() {
    let mock = MockServer::start().await;
    mock_server_with_instance_live_until(&mock, 5_600_000, 5_400_000).await;

    let contract_strkey = stellar_strkey::Contract([0u8; 32]).to_string();

    let mut cmd = Command::cargo_bin("sentinel").unwrap();
    cmd.args([
        "scan",
        &contract_strkey,
        "--rpc-url",
        &mock.uri(),
        "--format",
        "junit",
    ]);

    let assert = cmd.assert().code(0);
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();

    // Valid JUnit XML structure.
    assert!(stdout.starts_with("<?xml"));
    assert!(stdout.contains("<testsuites>"));
    assert!(stdout.contains("<testsuite"));
    assert!(stdout.contains("<testcase"));
    assert!(stdout.contains(&contract_strkey));
    assert!(stdout.contains("instance"));

    // OK entry: no <failure> child.
    assert!(!stdout.contains("<failure>"));
    assert!(stdout.contains("status=\"passed\""));
    assert!(stdout.contains("failures=\"0\""));
    assert!(stdout.contains("warnings=\"0\""));
}

#[tokio::test]
async fn junit_format_outputs_failure_for_critical_entry() {
    let mock = MockServer::start().await;
    // live_until 5,410,000 -> 10,000 ledgers remaining at latest 5,400,000 -> CRITICAL
    mock_server_with_instance_live_until(&mock, 5_410_000, 5_400_000).await;

    let contract_strkey = stellar_strkey::Contract([0u8; 32]).to_string();

    let mut cmd = Command::cargo_bin("sentinel").unwrap();
    cmd.args([
        "scan",
        &contract_strkey,
        "--rpc-url",
        &mock.uri(),
        "--format",
        "junit",
    ]);

    let assert = cmd.assert().code(2);
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();

    assert!(stdout.contains("<testsuites>"));
    assert!(stdout.contains("<testcase"));
    assert!(stdout.contains("<failure>"));
    assert!(stdout.contains("status=\"failed\""));
    assert!(stdout.contains("failures=\"1\""));
    assert!(stdout.contains("warnings=\"0\""));
}

#[tokio::test]
async fn junit_format_outputs_failure_for_archived_entry() {
    let mock = MockServer::start().await;
    // live_until 0 -> entry has no live-until -> ARCHIVED (live_until_ledger=0)
    mock_server_with_instance_live_until(&mock, 0, 5_400_000).await;

    let contract_strkey = stellar_strkey::Contract([0u8; 32]).to_string();

    let mut cmd = Command::cargo_bin("sentinel").unwrap();
    cmd.args([
        "scan",
        &contract_strkey,
        "--rpc-url",
        &mock.uri(),
        "--format",
        "junit",
    ]);

    let assert = cmd.assert().code(2);
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();

    assert!(stdout.contains("<failure>"));
    assert!(stdout.contains("status=\"failed\""));
    assert!(stdout.contains("is archived"));
}

#[tokio::test]
async fn junit_format_outputs_warning_for_near_expiry_entry() {
    let mock = MockServer::start().await;
    // live_until 5,450,000 -> 50,000 ledgers remaining at latest 5,400,000 -> WARN
    mock_server_with_instance_live_until(&mock, 5_450_000, 5_400_000).await;

    let contract_strkey = stellar_strkey::Contract([0u8; 32]).to_string();

    let mut cmd = Command::cargo_bin("sentinel").unwrap();
    cmd.args([
        "scan",
        &contract_strkey,
        "--rpc-url",
        &mock.uri(),
        "--format",
        "junit",
    ]);

    let assert = cmd.assert().code(1);
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();

    assert!(stdout.contains("<testsuites>"));
    assert!(stdout.contains("<testcase"));
    // Warning entries are not <failure> children in this formatter;
    // they are reflected via the suite warnings count and status="warning".
    assert!(stdout.contains("status=\"warning\""));
    assert!(stdout.contains("failures=\"0\""));
    assert!(stdout.contains("warnings=\"1\""));
}

#[test]
fn junit_format_is_advertised_in_scan_help() {
    let mut cmd = Command::cargo_bin("sentinel").unwrap();
    cmd.args(["scan", "--help"]);

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("junit"));
}

