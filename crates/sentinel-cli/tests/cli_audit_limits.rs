use assert_cmd::Command;
use predicates::prelude::*;
use stellar_xdr::curr::{
    ContractCodeEntry, ContractCodeEntryExt, ContractDataDurability, ContractDataEntry,
    ContractExecutable, ExtensionPoint, Hash, LedgerEntryData, Limits, ScAddress,
    ScContractInstance, ScVal, WriteXdr,
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

fn make_wasm_instance_xdr(contract_id: [u8; 32], wasm_hash: [u8; 32]) -> String {
    LedgerEntryData::ContractData(ContractDataEntry {
        ext: ExtensionPoint::V0,
        contract: ScAddress::Contract(Hash(contract_id)),
        key: ScVal::LedgerKeyContractInstance,
        durability: ContractDataDurability::Persistent,
        val: ScVal::ContractInstance(ScContractInstance {
            executable: ContractExecutable::Wasm(Hash(wasm_hash)),
            storage: None,
        }),
    })
    .to_xdr_base64(Limits::none())
    .unwrap()
}

fn make_contract_code_xdr(wasm_hash: [u8; 32], byte_len: usize) -> String {
    LedgerEntryData::ContractCode(ContractCodeEntry {
        ext: ContractCodeEntryExt::V0,
        hash: Hash(wasm_hash),
        code: vec![0u8; byte_len].try_into().unwrap(),
    })
    .to_xdr_base64(Limits::none())
    .unwrap()
}

fn make_persistent_xdr(contract_id: [u8; 32], byte_size: usize) -> String {
    // Generate dummy payload of approximately byte_size
    let dummy_bytes = vec![0u8; byte_size.saturating_sub(60)];
    let val = ScVal::Bytes(dummy_bytes.try_into().unwrap_or_default());
    LedgerEntryData::ContractData(ContractDataEntry {
        ext: ExtensionPoint::V0,
        contract: ScAddress::Contract(Hash(contract_id)),
        key: ScVal::Symbol("DATA".try_into().unwrap()),
        durability: ContractDataDurability::Persistent,
        val,
    })
    .to_xdr_base64(Limits::none())
    .unwrap()
}

#[tokio::test]
async fn test_audit_limits_compliant_contract_passes() {
    let mock = MockServer::start().await;
    let contract_id = [10u8; 32];
    let contract = stellar_strkey::Contract(contract_id).to_string();
    let instance_key = sentinel_core::derive_contract_instance_key(&contract_id).unwrap();

    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(move |req: &wiremock::Request| {
            let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
            match body["method"].as_str().unwrap() {
                "getLatestLedger" => ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "result": {"sequence": 5_400_000, "protocolVersion": 22}
                })),
                "getLedgerEntries" => ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "result": {
                        "latestLedger": 5_400_000,
                        "entries": [{
                            "key": instance_key,
                            "xdr": make_instance_xdr(contract_id),
                            "liveUntilLedgerSeq": 5_500_000,
                            "lastModifiedLedgerSeq": 5_390_000
                        }]
                    }
                })),
                _ => ResponseTemplate::new(200).set_body_json(
                    serde_json::json!({"jsonrpc": "2.0", "id": 1, "result": {"entries": []}}),
                ),
            }
        })
        .mount(&mock)
        .await;

    Command::cargo_bin("sentinel")
        .unwrap()
        .args([
            "audit-limits",
            &contract,
            "--rpc-url",
            &mock.uri(),
            "--format",
            "table",
        ])
        .assert()
        .code(0)
        .stdout(predicate::str::contains(
            "Storage Protocol Ceilings Scorecard",
        ))
        .stdout(predicate::str::contains("PASS"))
        .stdout(predicate::str::contains("ContractInstance"));
}

#[tokio::test]
async fn test_audit_limits_warning_threshold_exceeded() {
    let mock = MockServer::start().await;
    let contract_id = [11u8; 32];
    let contract = stellar_strkey::Contract(contract_id).to_string();
    let instance_key = sentinel_core::derive_contract_instance_key(&contract_id).unwrap();
    let persistent_key = sentinel_core::derive_persistent_symbol_key(&contract_id, "DATA").unwrap();

    // 55_000 bytes is > 80% of 65_536 (52_428 bytes), so it triggers WARN
    let warn_xdr = make_persistent_xdr(contract_id, 55_000);

    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(move |req: &wiremock::Request| {
            let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
            match body["method"].as_str().unwrap() {
                "getLatestLedger" => ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "result": {"sequence": 5_400_000, "protocolVersion": 22}
                })),
                "getLedgerEntries" => ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "result": {
                        "latestLedger": 5_400_000,
                        "entries": [
                            {
                                "key": instance_key,
                                "xdr": make_instance_xdr(contract_id),
                                "liveUntilLedgerSeq": 5_500_000,
                                "lastModifiedLedgerSeq": 5_390_000
                            },
                            {
                                "key": persistent_key,
                                "xdr": warn_xdr,
                                "liveUntilLedgerSeq": 5_500_000,
                                "lastModifiedLedgerSeq": 5_390_000
                            }
                        ]
                    }
                })),
                _ => ResponseTemplate::new(200).set_body_json(
                    serde_json::json!({"jsonrpc": "2.0", "id": 1, "result": {"entries": []}}),
                ),
            }
        })
        .mount(&mock)
        .await;

    Command::cargo_bin("sentinel")
        .unwrap()
        .args([
            "audit-limits",
            &contract,
            "--rpc-url",
            &mock.uri(),
            "--key",
            "DATA",
        ])
        .assert()
        .code(0) // Warning does not fail CI, exit code is 0
        .stdout(predicate::str::contains("WARN"))
        .stdout(predicate::str::contains("Approaching ceiling"));
}

#[tokio::test]
async fn test_audit_limits_ceiling_breach_returns_exit_code_2() {
    let mock = MockServer::start().await;
    let contract_id = [12u8; 32];
    let contract = stellar_strkey::Contract(contract_id).to_string();
    let instance_key = sentinel_core::derive_contract_instance_key(&contract_id).unwrap();
    let persistent_key = sentinel_core::derive_persistent_symbol_key(&contract_id, "DATA").unwrap();

    // 70_000 bytes breaches the 64 KB (65_536 bytes) network ceiling -> FAIL
    let breach_xdr = make_persistent_xdr(contract_id, 70_000);

    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(move |req: &wiremock::Request| {
            let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
            match body["method"].as_str().unwrap() {
                "getLatestLedger" => ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "result": {"sequence": 5_400_000, "protocolVersion": 22}
                })),
                "getLedgerEntries" => ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "result": {
                        "latestLedger": 5_400_000,
                        "entries": [
                            {
                                "key": instance_key,
                                "xdr": make_instance_xdr(contract_id),
                                "liveUntilLedgerSeq": 5_500_000,
                                "lastModifiedLedgerSeq": 5_390_000
                            },
                            {
                                "key": persistent_key,
                                "xdr": breach_xdr,
                                "liveUntilLedgerSeq": 5_500_000,
                                "lastModifiedLedgerSeq": 5_390_000
                            }
                        ]
                    }
                })),
                _ => ResponseTemplate::new(200).set_body_json(
                    serde_json::json!({"jsonrpc": "2.0", "id": 1, "result": {"entries": []}}),
                ),
            }
        })
        .mount(&mock)
        .await;

    // Must return exit code 2 if any ceiling is breached
    Command::cargo_bin("sentinel")
        .unwrap()
        .args([
            "audit-limits",
            &contract,
            "--rpc-url",
            &mock.uri(),
            "--key",
            "DATA",
        ])
        .assert()
        .code(2)
        .stdout(predicate::str::contains("FAIL"))
        .stdout(predicate::str::contains("BREACHED"));
}

#[tokio::test]
async fn test_audit_limits_checks_wasm_payload_size() {
    let mock = MockServer::start().await;
    let contract_id = [14u8; 32];
    let wasm_hash = [15u8; 32];
    let contract = stellar_strkey::Contract(contract_id).to_string();
    let instance_key = sentinel_core::derive_contract_instance_key(&contract_id).unwrap();
    let code_key = sentinel_core::derive_contract_code_key(&wasm_hash).unwrap();
    let instance_xdr = make_wasm_instance_xdr(contract_id, wasm_hash);
    let code_xdr = make_contract_code_xdr(wasm_hash, 70_000);

    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(move |req: &wiremock::Request| {
            let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
            match body["method"].as_str().unwrap() {
                "getLatestLedger" => ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "result": {"sequence": 5_400_000, "protocolVersion": 22}
                })),
                "getLedgerEntries" => {
                    let requested_keys = body["params"]["keys"].as_array().unwrap();
                    let (key, xdr) = if requested_keys
                        .iter()
                        .any(|key| key.as_str() == Some(code_key.as_str()))
                    {
                        (&code_key, &code_xdr)
                    } else {
                        (&instance_key, &instance_xdr)
                    };
                    ResponseTemplate::new(200).set_body_json(serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": 1,
                        "result": {
                            "latestLedger": 5_400_000,
                            "entries": [{
                                "key": key,
                                "xdr": xdr,
                                "liveUntilLedgerSeq": 5_500_000,
                                "lastModifiedLedgerSeq": 5_390_000
                            }]
                        }
                    }))
                }
                _ => ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "result": {"entries": []}
                })),
            }
        })
        .mount(&mock)
        .await;

    Command::cargo_bin("sentinel")
        .unwrap()
        .args(["audit-limits", &contract, "--rpc-url", &mock.uri()])
        .assert()
        .code(2)
        .stdout(predicate::str::contains("ContractCode (WASM)"))
        .stdout(predicate::str::contains("FAIL"));
}

#[tokio::test]
async fn test_audit_limits_json_format() {
    let mock = MockServer::start().await;
    let contract_id = [13u8; 32];
    let contract = stellar_strkey::Contract(contract_id).to_string();
    let instance_key = sentinel_core::derive_contract_instance_key(&contract_id).unwrap();

    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(move |req: &wiremock::Request| {
            let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
            match body["method"].as_str().unwrap() {
                "getLatestLedger" => ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "result": {"sequence": 5_400_000, "protocolVersion": 22}
                })),
                "getLedgerEntries" => ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "result": {
                        "latestLedger": 5_400_000,
                        "entries": [{
                            "key": instance_key,
                            "xdr": make_instance_xdr(contract_id),
                            "liveUntilLedgerSeq": 5_500_000,
                            "lastModifiedLedgerSeq": 5_390_000
                        }]
                    }
                })),
                _ => ResponseTemplate::new(200).set_body_json(
                    serde_json::json!({"jsonrpc": "2.0", "id": 1, "result": {"entries": []}}),
                ),
            }
        })
        .mount(&mock)
        .await;

    let output = Command::cargo_bin("sentinel")
        .unwrap()
        .args([
            "audit-limits",
            &contract,
            "--rpc-url",
            &mock.uri(),
            "--format",
            "json",
        ])
        .assert()
        .code(0)
        .get_output()
        .stdout
        .clone();

    let json: serde_json::Value = serde_json::from_slice(&output).expect("valid JSON scorecard");
    assert_eq!(json["contract_id"], contract);
    assert_eq!(json["overall_status"], "WARN");
    assert!(json["rows"]
        .as_array()
        .unwrap()
        .iter()
        .any(|row| row["notes"]
            .as_str()
            .unwrap_or_default()
            .contains("No persistent keys selected")));
    assert!(json["rows"].as_array().unwrap().len() >= 2);
}

#[test]
fn test_audit_limits_invalid_contract_id() {
    Command::cargo_bin("sentinel")
        .unwrap()
        .args(["audit-limits", "invalid_contract_id"])
        .assert()
        .code(10)
        .stderr(predicate::str::contains("Error"));
}
