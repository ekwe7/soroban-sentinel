use assert_cmd::Command;
use predicates::prelude::*;
use sentinel_core::adapters::{derive_contract_instance_key, derive_persistent_symbol_key};
use stellar_xdr::curr::{
    ContractDataDurability, ContractDataEntry, ContractExecutable, ExtensionPoint, Hash,
    LedgerEntryData, Limits, ScAddress, ScContractInstance, ScSymbol, ScVal, WriteXdr,
};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

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

fn make_persistent_symbol_xdr(contract_id: [u8; 32], symbol: &str) -> String {
    LedgerEntryData::ContractData(ContractDataEntry {
        ext: ExtensionPoint::V0,
        contract: ScAddress::Contract(Hash(contract_id)),
        key: ScVal::Symbol(ScSymbol(symbol.try_into().unwrap())),
        durability: ContractDataDurability::Persistent,
        val: ScVal::U32(42),
    })
    .to_xdr_base64(Limits::none())
    .unwrap()
}

#[tokio::test]
async fn test_cli_scan_ndjson_single_entry_valid_json_line() {
    let mock_server = MockServer::start().await;
    let contract_id = [21u8; 32];
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
        "--network",
        "testnet",
        "--format",
        "ndjson",
    ]);

    let output = cmd.output().unwrap();
    assert_eq!(output.status.code(), Some(0));

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(!stdout.contains("\x1b["));

    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 1, "Expected 1 NDJSON line, got: {stdout}");

    let entry: serde_json::Value =
        serde_json::from_str(lines[0]).expect("NDJSON line must parse as valid JSON");
    assert_eq!(entry["contract_id"], contract_strkey);
    assert_eq!(entry["network"], "testnet");
    assert_eq!(entry["latest_ledger"], 5_400_000);
    assert_eq!(entry["identifier"], "instance");
    assert_eq!(entry["entry_type"], "contract_instance");
    assert_eq!(entry["durability"], "instance");
    assert_eq!(entry["live_until_ledger"], 5_600_000);
    assert_eq!(entry["remaining_ledgers"], 200_000);
    assert_eq!(entry["status"], "ok");
}

#[tokio::test]
async fn test_cli_scan_ndjson_multiple_entries_line_by_line_parsing() {
    let mock_server = MockServer::start().await;
    let contract_id = [22u8; 32];
    let contract_strkey = stellar_strkey::Contract(contract_id).to_string();
    let instance_key = derive_contract_instance_key(&contract_id).unwrap();
    let admin_key = derive_persistent_symbol_key(&contract_id, "ADMIN").unwrap();
    let counter_key = derive_persistent_symbol_key(&contract_id, "COUNTER").unwrap();

    let instance_xdr = make_instance_xdr(contract_id);
    let admin_xdr = make_persistent_symbol_xdr(contract_id, "ADMIN");
    let counter_xdr = make_persistent_symbol_xdr(contract_id, "COUNTER");

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
                    let mut entries = Vec::new();
                    for key in keys {
                        let k = key.as_str().unwrap();
                        if k == instance_key {
                            entries.push(serde_json::json!({
                                "key": k,
                                "xdr": instance_xdr,
                                "liveUntilLedgerSeq": 5_600_000,
                                "lastModifiedLedgerSeq": 5_390_000
                            }));
                        } else if k == admin_key {
                            entries.push(serde_json::json!({
                                "key": k,
                                "xdr": admin_xdr,
                                "liveUntilLedgerSeq": 5_500_000,
                                "lastModifiedLedgerSeq": 5_390_000
                            }));
                        } else if k == counter_key {
                            entries.push(serde_json::json!({
                                "key": k,
                                "xdr": counter_xdr,
                                "liveUntilLedgerSeq": 5_410_000,
                                "lastModifiedLedgerSeq": 5_390_000
                            }));
                        }
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
        "--network",
        "mainnet",
        "--key",
        "ADMIN",
        "--key",
        "COUNTER",
        "--format",
        "ndjson",
    ]);

    let output = cmd.output().unwrap();
    // COUNTER has 10,000 ledgers remaining -> CRITICAL -> exit code 2
    assert_eq!(output.status.code(), Some(2));

    let stdout = String::from_utf8(output.stdout).unwrap();
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 3, "Expected 3 NDJSON lines, got:\n{stdout}");

    let parsed: Vec<serde_json::Value> = lines
        .iter()
        .map(|line| {
            assert!(!line.is_empty());
            serde_json::from_str(line).expect("Every NDJSON line must be valid JSON")
        })
        .collect();

    for record in &parsed {
        assert_eq!(record["contract_id"], contract_strkey);
        assert_eq!(record["network"], "mainnet");
        assert_eq!(record["latest_ledger"], 5_400_000);
        assert!(record["live_until_ledger"].is_u64());
        assert!(record["remaining_ledgers"].is_i64());
        assert!(record["status"].is_string());
    }

    assert_eq!(parsed[0]["identifier"], "instance");
    assert_eq!(parsed[0]["status"], "ok");

    assert_eq!(parsed[1]["identifier"], "persistent:ADMIN");
    assert_eq!(parsed[1]["status"], "warn");

    assert_eq!(parsed[2]["identifier"], "persistent:COUNTER");
    assert_eq!(parsed[2]["status"], "critical");
}

#[tokio::test]
async fn test_cli_scan_ndjson_writes_valid_lines_to_output_file() {
    let mock_server = MockServer::start().await;
    let contract_id = [23u8; 32];
    let contract_strkey = stellar_strkey::Contract(contract_id).to_string();
    let instance_key = derive_contract_instance_key(&contract_id).unwrap();
    let admin_key = derive_persistent_symbol_key(&contract_id, "ADMIN").unwrap();
    let instance_xdr = make_instance_xdr(contract_id);
    let admin_xdr = make_persistent_symbol_xdr(contract_id, "ADMIN");

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
                    let mut entries = Vec::new();
                    for key in keys {
                        let k = key.as_str().unwrap();
                        if k == instance_key {
                            entries.push(serde_json::json!({
                                "key": k,
                                "xdr": instance_xdr,
                                "liveUntilLedgerSeq": 5_600_000,
                                "lastModifiedLedgerSeq": 5_390_000
                            }));
                        } else if k == admin_key {
                            entries.push(serde_json::json!({
                                "key": k,
                                "xdr": admin_xdr,
                                "liveUntilLedgerSeq": 5_600_000,
                                "lastModifiedLedgerSeq": 5_390_000
                            }));
                        }
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

    let out_path = std::env::temp_dir().join(format!(
        "sentinel_ndjson_output_{}_{}.ndjson",
        std::process::id(),
        contract_strkey
    ));
    let _ = std::fs::remove_file(&out_path);

    let mut cmd = Command::cargo_bin("sentinel").unwrap();
    cmd.args([
        "scan",
        &contract_strkey,
        "--rpc-url",
        &mock_server.uri(),
        "--key",
        "ADMIN",
        "--format",
        "ndjson",
        "--output",
        out_path.to_str().unwrap(),
    ]);

    cmd.assert().code(0).stdout(predicate::str::is_empty());

    let contents = std::fs::read_to_string(&out_path).unwrap();
    let lines: Vec<&str> = contents.lines().collect();
    assert_eq!(lines.len(), 2);
    for line in lines {
        let parsed: serde_json::Value =
            serde_json::from_str(line).expect("Output file line must be valid JSON");
        assert_eq!(parsed["contract_id"], contract_strkey);
        assert_eq!(parsed["latest_ledger"], 5_400_000);
    }

    let _ = std::fs::remove_file(&out_path);
}

#[tokio::test]
async fn test_cli_scan_no_color_flag_emits_zero_ansi_codes() {
    let mock_server = MockServer::start().await;
    let contract_id = [24u8; 32];
    let contract_strkey = stellar_strkey::Contract(contract_id).to_string();
    let instance_key = derive_contract_instance_key(&contract_id).unwrap();
    let admin_key = derive_persistent_symbol_key(&contract_id, "ADMIN").unwrap();
    let instance_xdr = make_instance_xdr(contract_id);
    let admin_xdr = make_persistent_symbol_xdr(contract_id, "ADMIN");

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
                    let mut entries = Vec::new();
                    for key in keys {
                        let k = key.as_str().unwrap();
                        if k == instance_key {
                            entries.push(serde_json::json!({
                                "key": k,
                                "xdr": instance_xdr,
                                "liveUntilLedgerSeq": 5_500_000,
                                "lastModifiedLedgerSeq": 5_390_000
                            }));
                        } else if k == admin_key {
                            entries.push(serde_json::json!({
                                "key": k,
                                "xdr": admin_xdr,
                                "liveUntilLedgerSeq": 5_410_000,
                                "lastModifiedLedgerSeq": 5_390_000
                            }));
                        }
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
    cmd.env_remove("NO_COLOR").args([
        "scan",
        &contract_strkey,
        "--rpc-url",
        &mock_server.uri(),
        "--key",
        "ADMIN",
        "--format",
        "text",
        "--no-color",
    ]);

    let output = cmd.output().unwrap();
    assert_eq!(output.status.code(), Some(2));

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        !stdout.contains("\x1b["),
        "Expected zero ANSI escape sequences with --no-color, got:\n{stdout}"
    );
    assert!(stdout.contains("Contract Instance"));
    assert!(stdout.contains("WARN"));
    assert!(stdout.contains("CRITICAL"));
}

#[tokio::test]
async fn test_cli_scan_no_color_env_emits_zero_ansi_codes() {
    let mock_server = MockServer::start().await;
    let contract_id = [25u8; 32];
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

    // 1. Non-empty NO_COLOR suppresses ANSI escape codes
    let mut cmd = Command::cargo_bin("sentinel").unwrap();
    cmd.env("NO_COLOR", "1").args([
        "scan",
        &contract_strkey,
        "--rpc-url",
        &mock_server.uri(),
        "--format",
        "text",
    ]);

    let output = cmd.output().unwrap();
    assert_eq!(output.status.code(), Some(1));

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        !stdout.contains("\x1b["),
        "Expected zero ANSI escape sequences with NO_COLOR=1, got:\n{stdout}"
    );
    assert!(stdout.contains("WARN"));

    // 2. Baseline diff with --no-color and NO_COLOR also emits zero ANSI escape codes
    let temp_baseline = std::env::temp_dir().join(format!(
        "sentinel_no_color_baseline_{}_{}.json",
        std::process::id(),
        contract_strkey
    ));
    let baseline_content = serde_json::json!({
        "contract_id": contract_strkey,
        "network": "testnet",
        "latest_ledger": 5_300_000,
        "entries": [{
            "identifier": "instance",
            "live_until_ledger": 5_500_000,
            "remaining_ledgers": 200_000,
            "status": "ok"
        }]
    });
    std::fs::write(&temp_baseline, baseline_content.to_string()).unwrap();

    for (use_flag, env_val) in [(true, None), (false, Some("1"))] {
        let mut diff_cmd = Command::cargo_bin("sentinel").unwrap();
        if let Some(val) = env_val {
            diff_cmd.env("NO_COLOR", val);
        } else {
            diff_cmd.env_remove("NO_COLOR");
        }
        diff_cmd.args([
            "scan",
            &contract_strkey,
            "--rpc-url",
            &mock_server.uri(),
            "--baseline",
            temp_baseline.to_str().unwrap(),
            "--format",
            "text",
        ]);
        if use_flag {
            diff_cmd.arg("--no-color");
        }

        let diff_out = diff_cmd.output().unwrap();
        assert_eq!(diff_out.status.code(), Some(1));
        let diff_stdout = String::from_utf8(diff_out.stdout).unwrap();
        assert!(
            !diff_stdout.contains("\x1b["),
            "Expected zero ANSI escape sequences in baseline diff output"
        );
        assert!(diff_stdout.contains("Baseline Comparison"));
        assert!(diff_stdout.contains("status_worsened"));
    }

    let _ = std::fs::remove_file(&temp_baseline);
}

#[test]
fn test_scan_help_advertises_ndjson_and_no_color() {
    let mut cmd = Command::cargo_bin("sentinel").unwrap();
    cmd.args(["scan", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("ndjson"))
        .stdout(predicate::str::contains("--no-color"));
}
