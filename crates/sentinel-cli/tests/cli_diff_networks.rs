use assert_cmd::Command;
use predicates::prelude::*;
use stellar_xdr::curr::{
    ContractDataDurability, ContractDataEntry, ContractExecutable, ExtensionPoint, Hash,
    LedgerEntryData, Limits, ScAddress, ScContractInstance, ScVal, WriteXdr,
};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

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

async fn mock_network(
    latest_ledger: u32,
    instance_live_until: u32,
    instance_xdr: String,
    omit_persistent_entry: bool,
) -> MockServer {
    let mock_server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(move |req: &wiremock::Request| {
            let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
            let requests = body.as_array().cloned().unwrap_or_else(|| vec![body.clone()]);
            let responses: Vec<_> = requests
                .iter()
                .map(|request| {
                    let id = request["id"].clone();
                    let result = match request["method"].as_str().unwrap() {
                        "getLatestLedger" => serde_json::json!({
                            "sequence": latest_ledger,
                            "protocolVersion": 22
                        }),
                        "getLedgers" => serde_json::json!({
                            "ledgers": [
                                { "sequence": latest_ledger.saturating_sub(1), "ledgerCloseTime": "1000" },
                                { "sequence": latest_ledger, "ledgerCloseTime": "1005" }
                            ]
                        }),
                        "getLedgerEntries" => {
                            let keys = request["params"]["keys"].as_array().unwrap();
                            let selected_keys = if omit_persistent_entry {
                                &keys[..keys.len().min(1)]
                            } else {
                                &keys[..]
                            };
                            let entries: Vec<_> = selected_keys
                                .iter()
                                .map(|key| {
                                    serde_json::json!({
                                        "key": key,
                                        "xdr": instance_xdr,
                                        "liveUntilLedgerSeq": instance_live_until,
                                        "lastModifiedLedgerSeq": latest_ledger.saturating_sub(1)
                                    })
                                })
                                .collect();
                            serde_json::json!({
                                "latestLedger": latest_ledger,
                                "entries": entries
                            })
                        }
                        other => panic!("Unexpected method: {}", other),
                    };
                    serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": result
                    })
                })
                .collect();
            let response = if body.is_array() {
                serde_json::Value::Array(responses)
            } else {
                responses.into_iter().next().unwrap()
            };
            ResponseTemplate::new(200).set_body_json(response)
        })
        .mount(&mock_server)
        .await;
    mock_server
}

#[tokio::test]
async fn identical_ttl_horizons_on_different_ledgers_return_zero() {
    let contract_id = [21u8; 32];
    let contract_strkey = stellar_strkey::Contract(contract_id).to_string();
    let instance_xdr = make_instance_xdr(contract_id);
    let network_a = mock_network(1_000, 2_000, instance_xdr.clone(), false).await;
    let network_b = mock_network(5_000, 6_000, instance_xdr, false).await;

    let mut cmd = Command::cargo_bin("sentinel").unwrap();
    cmd.args([
        "diff-networks",
        &contract_strkey,
        "--network-a",
        "testnet",
        "--network-b",
        "mainnet",
        "--rpc-url-a",
        &network_a.uri(),
        "--rpc-url-b",
        &network_b.uri(),
        "--ttl-drift-threshold",
        "10",
    ]);

    cmd.assert()
        .code(0)
        .stdout(predicate::str::contains("testnet TTL Horizon"))
        .stdout(predicate::str::contains("mainnet TTL Horizon"))
        .stdout(predicate::str::contains("instance"))
        .stdout(predicate::str::contains("OK"));
}

#[tokio::test]
async fn ttl_drift_and_missing_persistent_key_return_one() {
    let contract_id = [22u8; 32];
    let contract_strkey = stellar_strkey::Contract(contract_id).to_string();
    let instance_xdr = make_instance_xdr(contract_id);
    let network_a = mock_network(1_000, 2_000, instance_xdr.clone(), false).await;
    let network_b = mock_network(2_000, 2_300, instance_xdr, true).await;

    let mut cmd = Command::cargo_bin("sentinel").unwrap();
    cmd.args([
        "diff-networks",
        &contract_strkey,
        "--network-a",
        "testnet",
        "--network-b",
        "mainnet",
        "--rpc-url-a",
        &network_a.uri(),
        "--rpc-url-b",
        &network_b.uri(),
        "--key",
        "COUNTER",
        "--ttl-drift-threshold",
        "100",
    ]);

    cmd.assert()
        .code(1)
        .stdout(predicate::str::contains("TTL DRIFT: 700 ledgers"))
        .stdout(predicate::str::contains("MISSING IN mainnet"))
        .stdout(predicate::str::contains("persistent:COUNTER"));
}
