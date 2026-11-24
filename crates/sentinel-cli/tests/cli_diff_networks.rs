use assert_cmd::Command;
use predicates::prelude::*;
use stellar_xdr::curr::{
    ContractDataDurability, ContractDataEntry, ContractExecutable, ExtensionPoint, Hash,
    LedgerEntryData, Limits, ScAddress, ScContractInstance, ScVal, WriteXdr,
};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

use sentinel_core::adapters::derive_contract_instance_key;

fn make_instance_xdr(contract_id: [u8; 32]) -> String {
    let data = LedgerEntryData::ContractData(ContractDataEntry {
        ext: ExtensionPoint::V0,
        contract: ScAddress::Contract(Hash(contract_id)),
        key: ScVal::LedgerKeyContractInstance,
        durability: ContractDataDurability::Persistent,
        val: ScVal::ContractInstance(ScContractInstance {
            executable: ContractExecutable::StellarAsset,
            storage: None,
        }),
    });
    data.to_xdr_base64(Limits::none()).unwrap()
}

async fn mock_network(
    contract_id: [u8; 32],
    latest_ledger: u32,
    live_until: u32,
    include_entry: bool,
) -> MockServer {
    let server = MockServer::start().await;
    let instance_key = derive_contract_instance_key(&contract_id).unwrap();
    let instance_xdr = make_instance_xdr(contract_id);
    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(move |request: &Request| {
            let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
            match body["method"].as_str().unwrap() {
                "getLatestLedger" => ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": body["id"],
                    "result": { "sequence": latest_ledger, "protocolVersion": 22 }
                })),
                "getLedgerEntries" => {
                    let entries = if include_entry {
                        vec![serde_json::json!({
                            "key": instance_key.clone(),
                            "xdr": instance_xdr.clone(),
                            "liveUntilLedgerSeq": live_until,
                            "lastModifiedLedgerSeq": latest_ledger.saturating_sub(100)
                        })]
                    } else {
                        Vec::new()
                    };
                    ResponseTemplate::new(200).set_body_json(serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": body["id"],
                        "result": { "entries": entries }
                    }))
                }
                "getLedgers" => ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": body["id"],
                    "result": { "ledgers": [] }
                })),
                other => panic!("unexpected RPC method: {other}"),
            }
        })
        .mount(&server)
        .await;
    server
}

fn diff_command(
    contract_id: &str,
    network_a: &str,
    network_b: &str,
    rpc_a: &str,
    rpc_b: &str,
) -> Command {
    let mut command = Command::cargo_bin("sentinel").unwrap();
    command.args([
        "diff-networks",
        contract_id,
        "--network-a",
        network_a,
        "--network-b",
        network_b,
        "--rpc-url-a",
        rpc_a,
        "--rpc-url-b",
        rpc_b,
    ]);
    command
}

#[tokio::test]
async fn diff_networks_reports_ttl_drift_and_returns_one() {
    let contract = [7; 32];
    let contract_id = stellar_strkey::Contract(contract).to_string();
    let a = mock_network(contract, 5_400_000, 5_600_000, true).await;
    let b = mock_network(contract, 5_400_000, 5_450_000, true).await;

    diff_command(&contract_id, "testnet", "mainnet", &a.uri(), &b.uri())
        .args(["--format", "json", "--ttl-drift-threshold", "100"])
        .assert()
        .code(1)
        .stdout(predicate::str::contains("\"significant_changes\": 1"))
        .stdout(predicate::str::contains("TTL drift"));
}

#[tokio::test]
async fn diff_networks_returns_zero_for_identical_health() {
    let contract = [8; 32];
    let contract_id = stellar_strkey::Contract(contract).to_string();
    let a = mock_network(contract, 5_400_000, 5_600_000, true).await;
    let b = mock_network(contract, 5_400_000, 5_600_000, true).await;

    diff_command(&contract_id, "testnet", "mainnet", &a.uri(), &b.uri())
        .assert()
        .code(0)
        .stdout(predicate::str::contains("within threshold"));
}

#[tokio::test]
async fn diff_networks_treats_different_risk_statuses_as_significant() {
    let contract = [10; 32];
    let contract_id = stellar_strkey::Contract(contract).to_string();
    let a = mock_network(contract, 5_400_000, 5_400_010, true).await;
    let b = mock_network(contract, 5_400_000, 5_450_000, true).await;

    diff_command(&contract_id, "testnet", "mainnet", &a.uri(), &b.uri())
        .args(["--format", "json", "--ttl-drift-threshold", "100000"])
        .assert()
        .code(1)
        .stdout(predicate::str::contains(
            "\"change\": \"risk status differs\"",
        ));
}

#[tokio::test]
async fn diff_networks_marks_keys_missing_on_one_network() {
    let contract = [9; 32];
    let contract_id = stellar_strkey::Contract(contract).to_string();
    let a = mock_network(contract, 5_400_000, 5_600_000, true).await;
    let b = mock_network(contract, 5_400_000, 0, false).await;

    diff_command(&contract_id, "testnet", "mainnet", &a.uri(), &b.uri())
        .assert()
        .code(1)
        .stdout(predicate::str::contains("missing from B"));
}
