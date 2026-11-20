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
    let _instance_key = derive_contract_instance_key(&contract_id).unwrap();
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
                "getLedgerEntries" => {
                    let keys = body["params"]["keys"].as_array().unwrap();
                    let mut entries = vec![];
                    for k in keys {
                        entries.push(serde_json::json!({
                            "key": k,
                            "xdr": instance_xdr,
                            "liveUntilLedgerSeq": 5_600_000,
                            "lastModifiedLedgerSeq": 5_390_000
                        }));
                    }
                    ResponseTemplate::new(200).set_body_json(serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": 1,
                        "result": {
                            "latestLedger": 5_400_000,
                            "entries": entries
                        }
                    }))
                }
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

// Test H: Malformed JSON from RPC endpoint -> asserts exit code 11 (EXIT_RPC_FAILURE)
#[tokio::test]
async fn test_cli_scan_malformed_rpc_json_returns_exit_code_11() {
    let mock_server = MockServer::start().await;
    let contract_id = [7u8; 32];
    let contract_strkey = stellar_strkey::Contract(contract_id).to_string();

    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string("THIS IS NOT JSON"))
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

    cmd.assert()
        .code(11)
        .stderr(predicate::str::contains(
            "Failed to parse JSON-RPC response",
        ))
        .stdout(predicate::str::is_empty());
}

// Test I: Missing required CONTRACT_ID -> asserts exit code 10 (EXIT_INVALID_ARGS)
#[test]
fn test_cli_scan_missing_contract_id_returns_exit_code_10() {
    let mut cmd = Command::cargo_bin("sentinel").unwrap();
    cmd.args(["scan"]);

    cmd.assert()
        .code(10)
        .stderr(predicate::str::contains("<CONTRACT_ID>"));
}

// Test J: CLI help and version flags continue to exit 0
#[test]
fn test_cli_help_and_version_flags_return_exit_code_0() {
    // Root --help
    let mut cmd = Command::cargo_bin("sentinel").unwrap();
    cmd.arg("--help");
    cmd.assert()
        .code(0)
        .stdout(predicate::str::contains("Usage:"));

    // Root --version
    let mut cmd = Command::cargo_bin("sentinel").unwrap();
    cmd.arg("--version");
    cmd.assert()
        .code(0)
        .stdout(predicate::str::contains("sentinel"));

    // Subcommand scan --help
    let mut cmd = Command::cargo_bin("sentinel").unwrap();
    cmd.args(["scan", "--help"]);
    cmd.assert()
        .code(0)
        .stdout(predicate::str::contains("<CONTRACT_ID>"));
}

// Test K: Critical risk entries -> asserts exit code 2 (EXIT_CRITICAL_OR_ARCHIVED)
#[tokio::test]
async fn test_cli_scan_critical_entries_returns_exit_code_2() {
    let mock_server = MockServer::start().await;
    let contract_id = [8u8; 32];
    let contract_strkey = stellar_strkey::Contract(contract_id).to_string();
    let instance_key = derive_contract_instance_key(&contract_id).unwrap();
    let instance_xdr = make_instance_xdr(contract_id);

    // Live until 5,410,000 -> 10,000 ledgers remaining (< 34,560) -> CRITICAL
    let live_until = 5_410_000;

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
        .code(2)
        .stdout(predicate::str::contains("CRITICAL"))
        .stdout(predicate::str::contains(
            "Summary: 1 entries checked. 0 OK, 0 WARN, 1 CRITICAL/ARCHIVED.",
        ));
}

// Test L: Scan with typed key expression --key-expr
#[tokio::test]
async fn test_cli_scan_with_key_expr_success() {
    let mock_server = MockServer::start().await;
    let contract_id = [9u8; 32];
    let contract_strkey = stellar_strkey::Contract(contract_id).to_string();
    let _instance_key = derive_contract_instance_key(&contract_id).unwrap();
    let instance_xdr = make_instance_xdr(contract_id);

    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(move |req: &wiremock::Request| {
            let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
            match body["method"].as_str().unwrap() {
                "getLatestLedger" => ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "result": { "sequence": 5_400_000, "protocolVersion": 22 }
                })),
                "getLedgerEntries" => {
                    let keys = body["params"]["keys"].as_array().unwrap();
                    let mut entries = vec![];
                    for k in keys {
                        entries.push(serde_json::json!({
                            "key": k,
                            "xdr": instance_xdr,
                            "liveUntilLedgerSeq": 5_600_000,
                            "lastModifiedLedgerSeq": 5_390_000
                        }));
                    }
                    ResponseTemplate::new(200).set_body_json(serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": 1,
                        "result": {
                            "latestLedger": 5_400_000,
                            "entries": entries
                        }
                    }))
                }
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
        "--key-expr",
        "sym:Balance",
        "--format",
        "json",
    ]);

    cmd.assert()
        .code(0)
        .stdout(predicate::str::contains("persistent:sym:Balance"));
}

// Test M: Scan with malformed --key-expr returns exit code 10
#[test]
fn test_cli_scan_malformed_key_expr_returns_exit_code_10() {
    let mut cmd = Command::cargo_bin("sentinel").unwrap();
    cmd.args([
        "scan",
        "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM",
        "--key-expr",
        "badprefix:123",
    ]);

    cmd.assert()
        .code(10)
        .stderr(predicate::str::contains("Unknown key expression prefix"));
}

// Test N: Invalid --rpc-timeout or --rpc-max-retries bounds return exit code 10
#[test]
fn test_cli_scan_invalid_rpc_bounds_returns_exit_code_10() {
    // Timeout 0
    let mut cmd = Command::cargo_bin("sentinel").unwrap();
    cmd.args([
        "scan",
        "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM",
        "--rpc-timeout",
        "0",
    ]);
    cmd.assert().code(10).stderr(predicate::str::contains(
        "--rpc-timeout must be between 1 and 120",
    ));

    // Retries 11
    let mut cmd2 = Command::cargo_bin("sentinel").unwrap();
    cmd2.args([
        "scan",
        "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM",
        "--rpc-max-retries",
        "15",
    ]);
    cmd2.assert().code(10).stderr(predicate::str::contains(
        "--rpc-max-retries must be between 0 and 10",
    ));
}

// Test O: Scan with --baseline comparison against wiremock
#[tokio::test]
async fn test_cli_scan_baseline_comparison() {
    let mock_server = MockServer::start().await;
    let contract_id = [10u8; 32];
    let contract_strkey = stellar_strkey::Contract(contract_id).to_string();
    let instance_key = derive_contract_instance_key(&contract_id).unwrap();
    let instance_xdr = make_instance_xdr(contract_id);

    // Current live_until = 5,450,000 (WARN status with 50,000 ledgers remaining)
    let current_live = 5_450_000;

    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(move |req: &wiremock::Request| {
            let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
            match body["method"].as_str().unwrap() {
                "getLatestLedger" => ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "result": { "sequence": 5_400_000, "protocolVersion": 22 }
                })),
                "getLedgerEntries" => ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "result": {
                        "latestLedger": 5_400_000,
                        "entries": [{
                            "key": instance_key,
                            "xdr": instance_xdr,
                            "liveUntilLedgerSeq": current_live,
                            "lastModifiedLedgerSeq": 5_390_000
                        }]
                    }
                })),
                other => panic!("Unexpected method: {}", other),
            }
        })
        .mount(&mock_server)
        .await;

    // Create a temporary baseline file where instance was OK
    let temp_baseline = std::env::temp_dir().join(format!("baseline_{}.json", contract_strkey));
    let baseline_content = serde_json::json!({
        "contract_id": contract_strkey,
        "network": "testnet",
        "latest_ledger": 5_300_000,
        "entries": [{
            "identifier": "instance",
            "live_until_ledger": 5_450_000,
            "remaining_ledgers": 150_000,
            "status": "ok"
        }]
    });
    std::fs::write(&temp_baseline, baseline_content.to_string()).unwrap();

    // 1. Text format shows change table and summary
    let mut cmd = Command::cargo_bin("sentinel").unwrap();
    cmd.args([
        "scan",
        &contract_strkey,
        "--rpc-url",
        &mock_server.uri(),
        "--baseline",
        temp_baseline.to_str().unwrap(),
        "--format",
        "text",
    ]);

    cmd.assert()
        .code(1) // overall status is WARN -> exit 1
        .stdout(predicate::str::contains("Baseline Comparison"))
        .stdout(predicate::str::contains("status_worsened"))
        .stdout(predicate::str::contains("Baseline Diff Summary"));

    // 2. With --fail-on-regression, status_worsened triggers exit code 2
    let mut cmd_fail = Command::cargo_bin("sentinel").unwrap();
    cmd_fail.args([
        "scan",
        &contract_strkey,
        "--rpc-url",
        &mock_server.uri(),
        "--baseline",
        temp_baseline.to_str().unwrap(),
        "--fail-on-regression",
    ]);

    cmd_fail.assert().code(2);

    // 3. JSON format outputs "changes" object
    let mut cmd_json = Command::cargo_bin("sentinel").unwrap();
    cmd_json.args([
        "scan",
        &contract_strkey,
        "--rpc-url",
        &mock_server.uri(),
        "--baseline",
        temp_baseline.to_str().unwrap(),
        "--format",
        "json",
    ]);

    cmd_json
        .assert()
        .code(1)
        .stdout(predicate::str::contains("\"changes\""))
        .stdout(predicate::str::contains("\"has_regression\": true"));

    let _ = std::fs::remove_file(temp_baseline);
}

mod common;

// Temporary keys (--temp-key / --temp-key-xdr) combined with persistent keys in one scan
#[tokio::test]
async fn test_cli_scan_temporary_keys_end_to_end() {
    use sentinel_core::adapters::{derive_persistent_symbol_key, derive_temporary_symbol_key};
    use stellar_xdr::curr::ScSymbol;

    let mock_server = MockServer::start().await;
    let contract_id = [70u8; 32];
    let contract_strkey = stellar_strkey::Contract(contract_id).to_string();
    let nonce = ScVal::Symbol(ScSymbol("NONCE".try_into().unwrap()));
    let missing_xdr = ScVal::U32(404).to_xdr_base64(Limits::none()).unwrap();

    common::mount_ledger(
        &mock_server,
        5_400_000,
        vec![
            (
                derive_contract_instance_key(&contract_id).unwrap(),
                common::instance_entry_xdr(contract_id, None),
                5_600_000,
            ),
            (
                derive_persistent_symbol_key(&contract_id, "NONCE").unwrap(),
                common::data_entry_xdr(
                    contract_id,
                    nonce.clone(),
                    ContractDataDurability::Persistent,
                ),
                5_600_000,
            ),
            (
                derive_temporary_symbol_key(&contract_id, "NONCE").unwrap(),
                common::data_entry_xdr(contract_id, nonce, ContractDataDurability::Temporary),
                5_600_000,
            ),
        ],
    )
    .await;

    let args = [
        "scan",
        &contract_strkey,
        "--rpc-url",
        &mock_server.uri(),
        "--key",
        "NONCE",
        "--temp-key",
        "NONCE",
        "--temp-key-xdr",
        &missing_xdr,
    ];

    Command::cargo_bin("sentinel")
        .unwrap()
        .args(args)
        .assert()
        .code(2)
        .stdout(predicate::str::contains("Persistent Key: NONCE"))
        .stdout(predicate::str::contains("Temporary Key: NONCE"))
        .stdout(predicate::str::contains(format!(
            "UNRECOVERABLE: Temporary Key (XDR): {} is missing or expired",
            missing_xdr
        )));

    let output = Command::cargo_bin("sentinel")
        .unwrap()
        .args(args)
        .args(["--format", "json"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let parsed: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let entries = parsed["entries"].as_array().unwrap();
    let find = |id: &str| entries.iter().find(|e| e["identifier"] == id).unwrap();
    assert_eq!(find("persistent:NONCE")["durability"], "persistent");
    let temp = find("temporary:NONCE");
    assert_eq!(temp["durability"], "temporary");
    assert_eq!(temp["status"], "ok");
    let missing = find(&format!("temporary_xdr:{}", missing_xdr));
    assert_eq!(missing["durability"], "temporary");
    assert_eq!(missing["status"], "archived");
    assert_eq!(missing["unrecoverable"], true);
}

// An unknown --format is rejected by clap with exit code 10 before any RPC request
#[tokio::test]
async fn test_cli_scan_unknown_format_rejected_before_rpc() {
    let mock_server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&mock_server)
        .await;
    let contract_strkey = stellar_strkey::Contract([71u8; 32]).to_string();

    Command::cargo_bin("sentinel")
        .unwrap()
        .args([
            "scan",
            &contract_strkey,
            "--rpc-url",
            &mock_server.uri(),
            "--format",
            "xml",
        ])
        .assert()
        .code(10)
        .stderr(predicate::str::contains("invalid value 'xml'"));

    mock_server.verify().await;
}

/// Serves an instance entry whose remaining TTL yields the given status.
async fn mount_instance_with_live_until(
    server: &MockServer,
    contract_id: [u8; 32],
    live_until: u32,
) {
    common::mount_ledger(
        server,
        5_400_000,
        vec![(
            derive_contract_instance_key(&contract_id).unwrap(),
            common::instance_entry_xdr(contract_id, None),
            live_until,
        )],
    )
    .await;
}

// Markdown and Prometheus formats use the same risk exit codes as text and JSON
#[tokio::test]
async fn test_cli_scan_markdown_and_prometheus_exit_codes() {
    // 200,000 remaining -> OK (0); 100,000 -> WARN (1); 10,000 -> CRITICAL (2)
    for (seed, live_until, expected_code, status, status_value) in [
        (72u8, 5_600_000, 0, "OK", "0"),
        (73u8, 5_500_000, 1, "WARN", "1"),
        (74u8, 5_410_000, 2, "CRITICAL", "2"),
    ] {
        let mock_server = MockServer::start().await;
        let contract_id = [seed; 32];
        let contract_strkey = stellar_strkey::Contract(contract_id).to_string();
        mount_instance_with_live_until(&mock_server, contract_id, live_until).await;

        let markdown = Command::cargo_bin("sentinel")
            .unwrap()
            .args([
                "scan",
                &contract_strkey,
                "--rpc-url",
                &mock_server.uri(),
                "--format",
                "markdown",
            ])
            .output()
            .unwrap();
        assert_eq!(markdown.status.code(), Some(expected_code));
        let stdout = String::from_utf8(markdown.stdout).unwrap();
        assert!(stdout.contains(&format!("**Contract:** {}", contract_strkey)));
        assert!(stdout.contains("| Entry | Durability | Size | Live Until | Remaining | Status |"));
        assert!(stdout.contains("| Contract Instance | Instance |"));
        assert!(stdout.contains(&format!(" | {} |", status)));
        assert!(stdout.contains("**Summary:** 1 entries checked."));
        assert!(!stdout.contains('\x1b'));

        let prometheus = Command::cargo_bin("sentinel")
            .unwrap()
            .args([
                "scan",
                &contract_strkey,
                "--rpc-url",
                &mock_server.uri(),
                "--format",
                "prometheus",
            ])
            .output()
            .unwrap();
        assert_eq!(prometheus.status.code(), Some(expected_code));
        let stdout = String::from_utf8(prometheus.stdout).unwrap();
        assert!(stdout.ends_with('\n'));
        assert!(stdout.contains("# TYPE soroban_sentinel_latest_ledger gauge"));
        assert!(stdout.contains(&format!(
            "soroban_sentinel_latest_ledger{{contract=\"{}\",network=\"testnet\"}} 5400000",
            contract_strkey
        )));
        assert!(stdout.contains(&format!(
            "soroban_sentinel_entry_live_until_ledger{{contract=\"{}\",network=\"testnet\",entry=\"instance\",durability=\"instance\"}} {}",
            contract_strkey, live_until
        )));
        assert!(stdout.contains(&format!(
            "soroban_sentinel_entry_status{{contract=\"{}\",network=\"testnet\",entry=\"instance\",durability=\"instance\"}} {}",
            contract_strkey, status_value
        )));
    }
}

/// Runs `sentinel scan` against `server` in the given output format.
fn scan_instance_storage_cmd(
    server: &MockServer,
    contract_strkey: &str,
    format: &str,
) -> std::process::Output {
    Command::cargo_bin("sentinel")
        .unwrap()
        .args([
            "scan",
            contract_strkey,
            "--rpc-url",
            &server.uri(),
            "--format",
            format,
        ])
        .output()
        .unwrap()
}

/// Instance entry XDR holding `keys` storage values of 1,000 bytes, and its serialized size.
fn instance_storage_xdr(contract_id: [u8; 32], keys: u32) -> (String, usize) {
    let xdr = common::instance_entry_xdr(contract_id, Some(common::storage_map(keys, 1_000)));
    let size = sentinel_core::adapters::extract_instance_storage_stats(&xdr)
        .unwrap()
        .entry_size_bytes;
    (xdr, size)
}

/// Serves the instance entry with a healthy TTL, plus the network entry-size limit if given.
async fn mount_instance_storage(
    server: &MockServer,
    contract_id: [u8; 32],
    instance_xdr: String,
    limit: Option<u32>,
) {
    let mut entries = vec![(
        derive_contract_instance_key(&contract_id).unwrap(),
        instance_xdr,
        5_900_000,
    )];
    entries.extend(limit.map(common::entry_size_limit_entry));
    common::mount_ledger(server, 5_400_000, entries).await;
}

// Instance storage well under the network limit -> OK, metrics in text and JSON
#[tokio::test]
async fn test_cli_scan_instance_storage_ok() {
    let mock_server = MockServer::start().await;
    let contract_id = [90u8; 32];
    let contract_strkey = stellar_strkey::Contract(contract_id).to_string();
    let (instance_xdr, _) = instance_storage_xdr(contract_id, 2);
    mount_instance_storage(&mock_server, contract_id, instance_xdr, Some(65_536)).await;

    let text = scan_instance_storage_cmd(&mock_server, &contract_strkey, "text");
    assert_eq!(text.status.code(), Some(0));
    let stdout = String::from_utf8(text.stdout).unwrap();
    assert!(stdout.contains("Instance storage: 2 keys,"));
    assert!(stdout.contains("of the 65,536 B network entry-size limit"));
    assert!(!stdout.contains("instance storage SIZE"));
    assert!(text.stderr.is_empty());

    let json = scan_instance_storage_cmd(&mock_server, &contract_strkey, "json");
    assert_eq!(json.status.code(), Some(0));
    let parsed: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    let metrics = &parsed["entries"][0]["instance_storage"];
    assert_eq!(metrics["key_count"], 2);
    assert_eq!(metrics["max_entry_size_bytes"], 65_536);
    assert_eq!(metrics["size_status"], "ok");
}

// Instance storage at ~80% of the limit -> WARN from size (exit 1) although TTL is healthy
#[tokio::test]
async fn test_cli_scan_instance_storage_warn() {
    let mock_server = MockServer::start().await;
    let contract_id = [91u8; 32];
    let contract_strkey = stellar_strkey::Contract(contract_id).to_string();
    let (instance_xdr, size) = instance_storage_xdr(contract_id, 60);
    let limit = (size * 5 / 4) as u32;
    mount_instance_storage(&mock_server, contract_id, instance_xdr, Some(limit)).await;

    let text = scan_instance_storage_cmd(&mock_server, &contract_strkey, "text");
    assert_eq!(text.status.code(), Some(1));
    let stdout = String::from_utf8(text.stdout).unwrap();
    assert!(stdout.contains("Instance storage: 60 keys,"));
    assert!(stdout.contains("(80.0%)"));
    assert!(stdout.contains("WARN (instance storage SIZE)"));
    assert!(stdout.contains("This status is caused by entry size, not TTL."));

    let json = scan_instance_storage_cmd(&mock_server, &contract_strkey, "json");
    assert_eq!(json.status.code(), Some(1));
    let parsed: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    let instance = &parsed["entries"][0];
    assert_eq!(instance["status"], "warn");
    assert_eq!(instance["instance_storage"]["size_status"], "warn");
    assert_eq!(instance["instance_storage"]["utilization_pct"], 80.0);
}

// Network limit unavailable -> scan succeeds, size and count reported, utilisation omitted
#[tokio::test]
async fn test_cli_scan_instance_storage_limit_unavailable() {
    let mock_server = MockServer::start().await;
    let contract_id = [92u8; 32];
    let contract_strkey = stellar_strkey::Contract(contract_id).to_string();
    let (instance_xdr, _) = instance_storage_xdr(contract_id, 60);
    mount_instance_storage(&mock_server, contract_id, instance_xdr, None).await;

    let text = scan_instance_storage_cmd(&mock_server, &contract_strkey, "text");
    assert_eq!(text.status.code(), Some(0));
    assert!(String::from_utf8(text.stdout)
        .unwrap()
        .contains("Instance storage: 60 keys,"));
    assert!(String::from_utf8(text.stderr)
        .unwrap()
        .contains("max_contract_data_entry_bytes limit could not be fetched"));

    let json = scan_instance_storage_cmd(&mock_server, &contract_strkey, "json");
    assert_eq!(json.status.code(), Some(0));
    let parsed: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    let metrics = &parsed["entries"][0]["instance_storage"];
    assert_eq!(metrics["key_count"], 60);
    assert!(metrics["entry_size_bytes"].as_u64().unwrap() > 60_000);
    assert!(metrics.get("utilization_pct").is_none());
    assert!(String::from_utf8(json.stderr)
        .unwrap()
        .contains("utilisation is omitted"));
}

// Invalid instance-size percentages are rejected with exit 10 before any RPC request
#[tokio::test]
async fn test_cli_scan_invalid_instance_size_pcts_rejected_before_rpc() {
    let mock_server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&mock_server)
        .await;
    let contract_strkey = stellar_strkey::Contract([93u8; 32]).to_string();

    for (warn, critical) in [
        ("90", "75"),
        ("0", "50"),
        ("75", "101"),
        ("80", "80"),
        ("75", "300"),
    ] {
        Command::cargo_bin("sentinel")
            .unwrap()
            .args([
                "scan",
                &contract_strkey,
                "--rpc-url",
                &mock_server.uri(),
                "--instance-size-warn-pct",
                warn,
                "--instance-size-critical-pct",
                critical,
            ])
            .assert()
            .code(10);
    }
    mock_server.verify().await;
}
