use assert_cmd::Command;
use sentinel_cli::commands::health_matrix::health_score;
use sentinel_core::adapters::derive_contract_instance_key;
use sentinel_core::domain::{Durability, EntryType, RiskLevel};
use sentinel_core::usecases::{StorageEntryReport, TTLReport};
use serde_json::{json, Value};
use stellar_xdr::curr::*;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

#[test]
fn score_uses_documented_weights_and_status_caps() {
    let mut report = TTLReport {
        contract_id: stellar_strkey::Contract([1; 32]).to_string(),
        network: "testnet".into(),
        latest_ledger: 100,
        ledger_velocity_seconds: 5.0,
        entries: vec![StorageEntryReport::new(
            "key",
            EntryType::PersistentData,
            Durability::Persistent,
            500,
            150,
            50,
            250.0,
            250.0 / 86400.0,
            RiskLevel::Warn,
        )],
        overall_status: RiskLevel::Warn,
        archival_recommendations: vec![],
        sac_asset_info: None,
        burn_rate: Default::default(),
        contract_code_hash: None,
        contract_bytecode_size: None,
        contract_upgrade: None,
    };
    // 60*(50/100) + 20*(1-500/1000) + 20*(1-1/10) = 58.
    assert_eq!(health_score(&report, 100, 10, 1000), 58);
    report.overall_status = RiskLevel::Critical;
    assert_eq!(health_score(&report, 100, 10, 1000), 40);
    report.overall_status = RiskLevel::Archived;
    assert_eq!(health_score(&report, 100, 10, 1000), 0);
}

async fn fleet() -> (MockServer, tempfile::NamedTempFile) {
    let server = MockServer::start().await;
    let values: Vec<Value> = [(1_u8, 201000), (2, 51000), (3, 10000)]
        .into_iter()
        .map(|(id, ttl)| {
            let data = LedgerEntryData::ContractData(ContractDataEntry {
                ext: ExtensionPoint::V0,
                contract: ScAddress::Contract(Hash([id; 32])),
                key: ScVal::LedgerKeyContractInstance,
                durability: ContractDataDurability::Persistent,
                val: ScVal::ContractInstance(ScContractInstance {
                    executable: ContractExecutable::StellarAsset,
                    storage: None,
                }),
            });
            json!({
                "key": derive_contract_instance_key(&[id; 32]).unwrap(),
                "xdr": data.to_xdr_base64(Limits::none()).unwrap(),
                "liveUntilLedgerSeq": ttl,
                "lastModifiedLedgerSeq": 900
            })
        })
        .collect();
    Mock::given(method("POST"))
        .respond_with(move |request: &wiremock::Request| {
            let call: Value = serde_json::from_slice(&request.body).unwrap();
            if call["method"] == "getLedgerEntries"
                && call["params"]["keys"]
                    .as_array()
                    .unwrap()
                    .contains(&json!(derive_contract_instance_key(&[4; 32]).unwrap()))
            {
                return ResponseTemplate::new(200).set_body_json(json!({
                    "jsonrpc": "2.0", "id": call["id"],
                    "error": {"code": -32602, "message": "fixture RPC failure"}
                }));
            }
            let result = match call["method"].as_str().unwrap() {
                "getLatestLedger" => json!({"sequence": 1000, "protocolVersion": 22}),
                "getLedgers" => json!({"ledgers": []}),
                "getLedgerEntries" => {
                    let keys = call["params"]["keys"].as_array().unwrap();
                    let entries: Vec<_> = values
                        .iter()
                        .filter(|entry| keys.contains(&entry["key"]))
                        .collect();
                    json!({"entries": entries, "latestLedger": 1000})
                }
                _ => panic!("unexpected method"),
            };
            ResponseTemplate::new(200).set_body_json(json!({
                "jsonrpc": "2.0", "id": call["id"], "result": result
            }))
        })
        .mount(&server)
        .await;
    let manifest = tempfile::NamedTempFile::new().unwrap();
    let mut text = format!("[defaults]\nrpc_url='{}'\n", server.uri());
    for (id, name) in [(1_u8, "Healthy"), (2, "Warning"), (3, "Critical")] {
        text.push_str(&format!(
            "[[contracts]]\nid='{}'\nname='{name}'\n",
            stellar_strkey::Contract([id; 32])
        ));
    }
    std::fs::write(manifest.path(), text).unwrap();
    (server, manifest)
}

#[tokio::test]
async fn binary_renders_all_columns_and_filters_failing_scores() {
    let (_server, manifest) = fleet().await;
    let output = Command::cargo_bin("sentinel")
        .unwrap()
        .args(["health-matrix", "--manifest"])
        .arg(manifest.path())
        .env_remove("SOROBAN_RPC_URL")
        .assert()
        .code(2)
        .get_output()
        .stdout
        .clone();
    let text = String::from_utf8(output).unwrap();
    for label in [
        "Contract",
        "Health Score",
        "Entries",
        "Closest Cliff",
        "Daily Rent Est",
        "Status",
        "Healthy",
        "Warning",
        "Critical",
        "n/a",
    ] {
        assert!(text.contains(label), "{text}");
    }
    let output = Command::cargo_bin("sentinel")
        .unwrap()
        .args(["health-matrix", "--manifest"])
        .arg(manifest.path())
        .args([
            "--min-score",
            "70",
            "--rent-fee-per-kib",
            "1000",
            "--rent-denominator",
            "10",
        ])
        .env_remove("SOROBAN_RPC_URL")
        .assert()
        .code(2)
        .get_output()
        .stdout
        .clone();
    let filtered = String::from_utf8(output).unwrap();
    assert!(!filtered.contains("Healthy"));
    assert!(filtered.contains("Warning"));
    assert!(filtered.contains("Critical"));
    assert!(filtered.contains("stroops"));
}

#[test]
fn invalid_score_filter_and_incomplete_rent_parameters_are_rejected() {
    Command::cargo_bin("sentinel")
        .unwrap()
        .args([
            "health-matrix",
            "--manifest",
            "missing",
            "--min-score",
            "101",
        ])
        .assert()
        .code(10);
    Command::cargo_bin("sentinel")
        .unwrap()
        .args([
            "health-matrix",
            "--manifest",
            "missing",
            "--rent-fee-per-kib",
            "1",
        ])
        .assert()
        .code(10);
}

#[tokio::test]
async fn rpc_failure_stays_visible_when_filter_hides_healthy_contracts() {
    let (_server, manifest) = fleet().await;
    use std::io::Write;
    let mut append = std::fs::OpenOptions::new()
        .append(true)
        .open(manifest.path())
        .unwrap();
    writeln!(
        append,
        "[[contracts]]\nid='{}'\nname='Unavailable'",
        stellar_strkey::Contract([4; 32])
    )
    .unwrap();
    let output = Command::cargo_bin("sentinel")
        .unwrap()
        .args(["health-matrix", "--manifest"])
        .arg(manifest.path())
        .args(["--min-score", "0"])
        .env_remove("SOROBAN_RPC_URL")
        .assert()
        .code(11)
        .get_output()
        .stdout
        .clone();
    let text = String::from_utf8(output).unwrap();
    assert!(text.contains("Unavailable"));
    assert!(text.contains("UNKNOWN"));
    assert!(!text.contains("Healthy"));
}
