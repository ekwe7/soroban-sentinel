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

async fn mock_contract() -> (MockServer, String, u64) {
    let server = MockServer::start().await;
    let contract = [21; 32];
    let key = derive_contract_instance_key(&contract).unwrap();
    let entry = LedgerEntryData::ContractData(ContractDataEntry {
        ext: ExtensionPoint::V0,
        contract: ScAddress::Contract(Hash(contract)),
        key: ScVal::LedgerKeyContractInstance,
        durability: ContractDataDurability::Persistent,
        val: ScVal::ContractInstance(ScContractInstance {
            executable: ContractExecutable::StellarAsset,
            storage: None,
        }),
    });
    let xdr = entry.to_xdr_base64(Limits::none()).unwrap();
    let bytes = entry.to_xdr(Limits::none()).unwrap().len() as u64;
    Mock::given(method("POST"))
        .respond_with(move |req: &wiremock::Request| {
            let body: Value = serde_json::from_slice(&req.body).unwrap();
            let result = match body["method"].as_str().unwrap() {
                "getLatestLedger" => json!({"sequence": 5_000_000, "protocolVersion": 23}),
                "getLedgerEntries" => json!({
                    "latestLedger": 5_000_000,
                    "entries": [{
                        "key": key, "xdr": xdr, "liveUntilLedgerSeq": 5_000_001
                    }]
                }),
                other => panic!("Unexpected RPC method: {other}"),
            };
            ResponseTemplate::new(200)
                .set_body_json(json!({"jsonrpc": "2.0", "id": body["id"], "result": result}))
        })
        .mount(&server)
        .await;
    (
        server,
        stellar_strkey::Contract(contract).to_string(),
        bytes,
    )
}

#[tokio::test]
async fn json_reports_custom_tiers_and_compares_equal_horizons() {
    let (server, contract, bytes) = mock_contract().await;
    let mut command = Command::cargo_bin("sentinel").unwrap();
    let output = command
        .args([
            "cost-matrix",
            &contract,
            "--rpc-url",
            &server.uri(),
            "--format",
            "json",
            "--tiers",
            "90,90,40",
            "--rent-rate-per-kib",
            "1024",
            "--persistent-rent-denominator",
            "1",
            "--transaction-fee",
            "100",
            "--key",
            "MISSING",
        ])
        .assert()
        .success();
    let matrix: Value = serde_json::from_slice(&output.get_output().stdout).unwrap();
    let horizon = 90 * 17_280;
    let rent = bytes * (horizon - 1);
    assert_eq!(matrix["contract_id"], contract);
    assert_eq!(matrix["ledger_seconds"], 5.0);
    assert_eq!(matrix["minimum_account_reserve_stroops"], 10_000_000);
    assert_eq!(matrix["minimum_account_reserve_xlm"], "1.0000000");
    assert_eq!(matrix["incremental_extension_reserve_stroops"], 0);
    assert_eq!(matrix["skipped_entries"], json!(["persistent:MISSING"]));
    assert_eq!(matrix["tiers"].as_array().unwrap().len(), 2);
    assert_eq!(matrix["tiers"][0]["days"], 40);
    assert_eq!(matrix["tiers"][1]["ledgers"], horizon);
    assert_eq!(matrix["tiers"][1]["rent_stroops"], rent);
    assert_eq!(matrix["tiers"][1]["total_stroops"], rent + 100);
    assert!(matrix["tiers"][1]["total_xlm"].is_string());
    assert_eq!(matrix["comparisons"][0]["short_transactions"], 3);
    assert_eq!(matrix["comparisons"][0]["long_transactions"], 1);
    assert_eq!(matrix["comparisons"][0]["short_total_stroops"], rent + 300);
    assert_eq!(matrix["comparisons"][0]["break_even_fee_stroops"], 0);
}

#[tokio::test]
async fn terminal_table_reports_default_tiers() {
    let (server, contract, _) = mock_contract().await;
    Command::cargo_bin("sentinel")
        .unwrap()
        .args(["cost-matrix", &contract, "--rpc-url", &server.uri()])
        .assert()
        .success()
        .stdout(predicate::str::contains("Minimum source account reserve"))
        .stdout(predicate::str::contains("Rent (stroops)"))
        .stdout(predicate::str::contains("Total (XLM)"))
        .stdout(predicate::str::contains("Break-even fee"))
        .stdout(predicate::str::contains("365"));
}

#[test]
fn zero_day_tier_is_rejected() {
    Command::cargo_bin("sentinel")
        .unwrap()
        .args(["cost-matrix", "invalid", "--tiers", "0,30"])
        .assert()
        .code(10)
        .stderr(predicate::str::contains("0"));
}
