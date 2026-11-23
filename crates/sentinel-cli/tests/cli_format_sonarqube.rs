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
async fn cli_scan_sonarqube_output_matches_generic_issue_schema() {
    let server = MockServer::start().await;
    let contract_id = [146u8; 32];
    let contract_strkey = stellar_strkey::Contract(contract_id).to_string();
    let instance_key = derive_contract_instance_key(&contract_id).unwrap();
    let persistent_key = derive_persistent_symbol_key(&contract_id, "COUNTER").unwrap();
    let instance_xdr = make_instance_xdr(contract_id);
    let persistent_xdr = make_persistent_symbol_xdr(contract_id, "COUNTER");

    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(move |request: &wiremock::Request| {
            let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
            match body["method"].as_str().unwrap() {
                "getLatestLedger" => ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": body["id"],
                    "result": { "sequence": 5_400_000, "protocolVersion": 22 }
                })),
                "getLedgerEntries" => ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": body["id"],
                    "result": {
                        "latestLedger": 5_400_000,
                        "entries": [
                            {
                                "key": instance_key,
                                "xdr": instance_xdr,
                                "liveUntilLedgerSeq": 5_500_000,
                                "lastModifiedLedgerSeq": 5_390_000
                            },
                            {
                                "key": persistent_key,
                                "xdr": persistent_xdr,
                                "liveUntilLedgerSeq": 5_420_000,
                                "lastModifiedLedgerSeq": 5_390_000
                            }
                        ]
                    }
                })),
                other => panic!("Unexpected RPC method: {other}"),
            }
        })
        .mount(&server)
        .await;

    let output = Command::cargo_bin("sentinel")
        .unwrap()
        .args([
            "scan",
            &contract_strkey,
            "--rpc-url",
            &server.uri(),
            "--format",
            "sonarqube",
            "--key",
            "COUNTER",
        ])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2));
    let stdout = String::from_utf8(output.stdout).unwrap();
    let report: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let rules = report["rules"].as_array().expect("rules must be an array");
    let issues = report["issues"]
        .as_array()
        .expect("issues must be an array");
    assert_eq!(rules.len(), 2);
    assert_eq!(issues.len(), 2);

    let warn_rule = rules
        .iter()
        .find(|rule| rule["id"] == "SENTINEL-TTL-WARN")
        .unwrap();
    assert_eq!(warn_rule["engineId"], "soroban-sentinel");
    assert_eq!(warn_rule["cleanCodeAttribute"], "COMPLETE");
    assert_eq!(warn_rule["type"], "CODE_SMELL");
    assert_eq!(warn_rule["severity"], "MAJOR");

    let critical_rule = rules
        .iter()
        .find(|rule| rule["id"] == "SENTINEL-TTL-CRITICAL")
        .unwrap();
    assert_eq!(critical_rule["engineId"], "soroban-sentinel");
    assert_eq!(critical_rule["cleanCodeAttribute"], "TRUSTWORTHY");
    assert_eq!(critical_rule["type"], "VULNERABILITY");
    assert_eq!(critical_rule["severity"], "CRITICAL");

    let warn_issue = issues
        .iter()
        .find(|issue| issue["ruleId"] == "SENTINEL-TTL-WARN")
        .unwrap();
    assert_eq!(warn_issue["engineId"], "soroban-sentinel");
    assert_eq!(warn_issue["type"], "CODE_SMELL");
    assert_eq!(warn_issue["severity"], "MAJOR");
    assert!(warn_issue["primaryLocation"]["message"]
        .as_str()
        .unwrap()
        .contains("100000 ledgers remaining"));
    assert!(warn_issue["primaryLocation"]["filePath"]
        .as_str()
        .unwrap()
        .starts_with("soroban/"));

    let critical_issue = issues
        .iter()
        .find(|issue| issue["ruleId"] == "SENTINEL-TTL-CRITICAL")
        .unwrap();
    assert_eq!(critical_issue["engineId"], "soroban-sentinel");
    assert_eq!(critical_issue["type"], "VULNERABILITY");
    assert_eq!(critical_issue["severity"], "CRITICAL");
    assert!(critical_issue["primaryLocation"]["message"]
        .as_str()
        .unwrap()
        .contains("20000 ledgers remaining"));
}

#[test]
fn sonarqube_format_is_advertised_in_help() {
    let mut command = Command::cargo_bin("sentinel").unwrap();
    command
        .args(["scan", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("sonarqube"));
}
