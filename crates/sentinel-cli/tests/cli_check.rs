use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use assert_cmd::Command;
use predicates::prelude::*;
use sentinel_core::adapters::derive_contract_instance_key;
use serde_json::{json, Value};
use stellar_xdr::curr::{
    ContractDataDurability, ContractDataEntry, ContractExecutable, ExtensionPoint, Hash,
    LedgerEntryData, Limits, ScAddress, ScContractInstance, ScVal, WriteXdr,
};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

static NEXT_MANIFEST: AtomicUsize = AtomicUsize::new(0);

struct ManifestFile(PathBuf);

impl ManifestFile {
    fn new(contents: String) -> Self {
        let file = std::env::temp_dir().join(format!(
            "sentinel-check-{}-{}.toml",
            std::process::id(),
            NEXT_MANIFEST.fetch_add(1, Ordering::Relaxed)
        ));
        fs::write(&file, contents).unwrap();
        Self(file)
    }

    fn command(&self) -> Command {
        let mut command = Command::cargo_bin("sentinel").unwrap();
        command
            .args(["check", "--config"])
            .arg(&self.0)
            .env_remove("SOROBAN_RPC_URL");
        command
    }
}

impl Drop for ManifestFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn contract(bytes: [u8; 32], name: &str) -> String {
    format!(
        "[[contracts]]\nid = '{}'\nname = '{name}'\n",
        stellar_strkey::Contract(bytes)
    )
}

fn instance_entry(id: [u8; 32], ttl: u32) -> Value {
    let xdr = LedgerEntryData::ContractData(ContractDataEntry {
        ext: ExtensionPoint::V0,
        contract: ScAddress::Contract(Hash(id)),
        key: ScVal::LedgerKeyContractInstance,
        durability: ContractDataDurability::Persistent,
        val: ScVal::ContractInstance(ScContractInstance {
            executable: ContractExecutable::StellarAsset,
            storage: None,
        }),
    })
    .to_xdr_base64(Limits::none())
    .unwrap();
    json!({
        "key": derive_contract_instance_key(&id).unwrap(),
        "xdr": xdr,
        "liveUntilLedgerSeq": ttl,
        "lastModifiedLedgerSeq": 900
    })
}

async fn mock_fleet() -> MockServer {
    let server = MockServer::start().await;
    let warning = instance_entry([1; 32], 51_000);
    let critical = instance_entry([2; 32], 10_000);
    let ok = instance_entry([4; 32], 200_000);
    Mock::given(method("POST"))
        .respond_with(move |req: &wiremock::Request| {
            let call: Value = serde_json::from_slice(&req.body).unwrap();
            let response = match call["method"].as_str().unwrap() {
                "getLatestLedger" => json!({"result": {"sequence": 1000, "protocolVersion": 22}}),
                "getLedgers" => json!({"result": {"ledgers": []}}),
                "getLedgerEntries" => {
                    let key = &call["params"]["keys"][0];
                    if let Some(entry) = [&warning, &critical, &ok]
                        .into_iter()
                        .find(|entry| &entry["key"] == key)
                    {
                        json!({"result": {"latestLedger": 1000, "entries": [entry]}})
                    } else {
                        json!({"error": {"code": -32602, "message": "mock RPC failure"}})
                    }
                }
                method => panic!("Unexpected method {method}"),
            };
            let mut response = response;
            response["jsonrpc"] = json!("2.0");
            response["id"] = call["id"].clone();
            ResponseTemplate::new(200).set_body_json(response)
        })
        .mount(&server)
        .await;
    server
}

#[tokio::test]
async fn text_prints_all_sections_and_returns_worst_risk() {
    let server = mock_fleet().await;
    let manifest = ManifestFile::new(format!(
        "[defaults]\nrpc_url = '{}'\n{}{}",
        server.uri(),
        contract([1; 32], "Token"),
        contract([2; 32], "Pool")
    ));
    manifest
        .command()
        .assert()
        .code(2)
        .stdout(predicate::str::contains("=== Token"))
        .stdout(predicate::str::contains("=== Pool"))
        .stdout(predicate::str::contains(
            "2 contracts, 0 OK, 1 WARN, 1 CRITICAL, 0 ARCHIVED, 0 FAILED",
        ));
}

#[tokio::test]
async fn json_reports_rpc_failure_and_continues_in_manifest_order() {
    let server = mock_fleet().await;
    let manifest = ManifestFile::new(format!(
        "{}{}{}{}",
        contract([3; 32], "Broken"),
        contract([2; 32], "Pool"),
        contract([1; 32], "Token"),
        contract([4; 32], "Governance")
    ));
    let mut command = manifest.command();
    let output = command
        .env("SOROBAN_RPC_URL", server.uri())
        .args(["--format", "json"])
        .assert()
        .code(11);
    let value: Value = serde_json::from_slice(&output.get_output().stdout).unwrap();
    assert_eq!(
        value["summary"],
        json!({
            "total": 4, "ok": 1, "warn": 1, "critical": 1, "archived": 0, "failed": 1
        })
    );
    let contracts = value["contracts"].as_array().unwrap();
    assert_eq!(contracts[0]["name"], "Broken");
    assert_eq!(contracts[0]["error"], "RPC request failed (code -32602)");
    assert!(contracts[0]["report"].is_null());
    assert_eq!(contracts[1]["status"], "CRITICAL");
    assert_eq!(contracts[2]["status"], "WARN");
    assert_eq!(contracts[3]["status"], "OK");
}

#[tokio::test]
async fn invalid_later_contract_prevents_all_rpc_calls() {
    let server = mock_fleet().await;
    let bad_id = stellar_strkey::Contract([2; 32]).to_string();
    let manifest = ManifestFile::new(format!(
        "[defaults]\nrpc_url = '{}'\n{}{}key_xdr = ['bad-xdr!']\n",
        server.uri(),
        contract([1; 32], "Token"),
        contract([2; 32], "Pool")
    ));
    manifest
        .command()
        .assert()
        .code(10)
        .stderr(predicate::str::contains(bad_id))
        .stderr(predicate::str::contains("key_xdr"));
    assert!(server.received_requests().await.unwrap().is_empty());
}
