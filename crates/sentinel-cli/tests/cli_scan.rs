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

// Test A: Valid scan returning mock OK entries -> asserts stdout table and exit code 0
#[tokio::test]
async fn test_cli_scan_ok_entries_returns_exit_code_0() {
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

// Test B: Valid scan returning mock WARN entries -> asserts exit code 1
#[tokio::test]
async fn test_cli_scan_warn_entries_returns_exit_code_1() {
    let mock_server = MockServer::start().await;
    let contract_id = [2u8; 32];
    let contract_strkey = stellar_strkey::Contract(contract_id).to_string();
    let instance_key = derive_contract_instance_key(&contract_id).unwrap();
    let instance_xdr = make_instance_xdr(contract_id);

    // Live until 5,500,000 -> 100,000 ledgers remaining (< 120,960 and >= 34,560) -> WARN
    let live_until = 5_500_000;

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
    cmd.args(["scan", &contract_strkey, "--rpc-url", &mock_server.uri()]);

    cmd.assert()
        .code(1)
        .stdout(predicate::str::contains("WARN"))
        .stdout(predicate::str::contains(
            "Summary: 1 entries checked. 0 OK, 1 WARN, 0 CRITICAL/ARCHIVED.",
        ));
}

// Test C: Valid scan with --format json -> parses stdout as valid JSON
#[tokio::test]
async fn test_cli_scan_json_format_outputs_valid_schema() {
    let mock_server = MockServer::start().await;
    let contract_id = [3u8; 32];
    let contract_strkey = stellar_strkey::Contract(contract_id).to_string();
    let instance_key = derive_contract_instance_key(&contract_id).unwrap();
    let instance_xdr = make_instance_xdr(contract_id);

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
                            "liveUntilLedgerSeq": 5_600_000,
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
        "json",
    ]);

    let assert = cmd.assert().code(0);
    let stdout_bytes = assert.get_output().stdout.clone();
    let parsed: serde_json::Value =
        serde_json::from_slice(&stdout_bytes).expect("Stdout must be valid JSON");

    assert_eq!(parsed["contract_id"], contract_strkey);
    assert_eq!(parsed["latest_ledger"], 5_400_000);
    assert_eq!(parsed["summary"]["total_entries"], 1);
    assert_eq!(parsed["summary"]["ok"], 1);
    assert_eq!(parsed["entries"].as_array().unwrap().len(), 1);
    assert_eq!(parsed["entries"][0]["identifier"], "instance");
}

// Test D: Malformed Strkey -> asserts stderr error message and exit code 10
#[test]
fn test_cli_scan_malformed_strkey_returns_exit_code_10() {
    let mut cmd = Command::cargo_bin("sentinel").unwrap();
    cmd.args(["scan", "INVALID_STRKEY_NOT_CONTRACT"]);

    cmd.assert()
        .code(10)
        .stderr(predicate::str::contains("Invalid contract address"));
}

// Test E: Unreachable RPC -> asserts exit code 11
#[test]
fn test_cli_scan_unreachable_rpc_returns_exit_code_11() {
    let contract_id = [4u8; 32];
    let contract_strkey = stellar_strkey::Contract(contract_id).to_string();

    let mut cmd = Command::cargo_bin("sentinel").unwrap();
    cmd.args(["scan", &contract_strkey, "--rpc-url", "http://127.0.0.1:1"]);

    cmd.assert()
        .code(11)
        .stderr(predicate::str::contains("Error"));
}

// Test F: Invalid RPC URL scheme -> asserts stderr error message and exit code 10
#[test]
fn test_cli_scan_invalid_rpc_scheme_returns_exit_code_10() {
    let contract_id = [5u8; 32];
    let contract_strkey = stellar_strkey::Contract(contract_id).to_string();

    let mut cmd = Command::cargo_bin("sentinel").unwrap();
    cmd.args(["scan", &contract_strkey, "--rpc-url", "file:///etc/passwd"]);

    cmd.assert()
        .code(10)
        .stderr(predicate::str::contains("Unsupported scheme"));
}

// Test G: Contradictory threshold arguments -> asserts exit code 10
#[test]
fn test_cli_scan_contradictory_thresholds_returns_exit_code_10() {
    let contract_id = [6u8; 32];
    let contract_strkey = stellar_strkey::Contract(contract_id).to_string();

    let mut cmd = Command::cargo_bin("sentinel").unwrap();
    cmd.args([
        "scan",
        &contract_strkey,
        "--threshold",
        "10000",
        "--critical-threshold",
        "20000",
    ]);

    cmd.assert()
        .code(10)
        .stderr(predicate::str::contains("strictly less than"));
}
