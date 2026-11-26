use assert_cmd::Command;
use serde_json::Value;
use stellar_xdr::curr::{
    ContractDataDurability, ContractDataEntry, ContractExecutable, ExtensionPoint, Hash,
    LedgerEntryData, Limits, ReadXdr, ScAddress, ScContractInstance, ScSymbol, ScVal, WriteXdr,
};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use sentinel_core::adapters::{
    derive_contract_instance_key, derive_persistent_symbol_key, derive_temporary_symbol_key,
};

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

fn make_temporary_xdr(contract_id: [u8; 32]) -> String {
    LedgerEntryData::ContractData(ContractDataEntry {
        ext: ExtensionPoint::V0,
        contract: ScAddress::Contract(Hash(contract_id)),
        key: ScVal::Symbol(ScSymbol("CACHE".try_into().unwrap())),
        durability: ContractDataDurability::Temporary,
        val: ScVal::U32(1),
    })
    .to_xdr_base64(Limits::none())
    .unwrap()
}

fn make_persistent_xdr(contract_id: [u8; 32]) -> String {
    LedgerEntryData::ContractData(ContractDataEntry {
        ext: ExtensionPoint::V0,
        contract: ScAddress::Contract(Hash(contract_id)),
        key: ScVal::Symbol(ScSymbol("ADMIN".try_into().unwrap())),
        durability: ContractDataDurability::Persistent,
        val: ScVal::U32(1),
    })
    .to_xdr_base64(Limits::none())
    .unwrap()
}

async fn mock_rpc(contract_id: [u8; 32]) -> (MockServer, usize, usize, usize) {
    let server = MockServer::start().await;
    let instance_key = derive_contract_instance_key(&contract_id).unwrap();
    let persistent_key = derive_persistent_symbol_key(&contract_id, "ADMIN").unwrap();
    let temporary_key = derive_temporary_symbol_key(&contract_id, "CACHE").unwrap();
    let instance_xdr = make_instance_xdr(contract_id);
    let persistent_xdr = make_persistent_xdr(contract_id);
    let temporary_xdr = make_temporary_xdr(contract_id);
    let instance_size = LedgerEntryData::from_xdr_base64(&instance_xdr, Limits::none())
        .unwrap()
        .to_xdr(Limits::none())
        .unwrap()
        .len();
    let persistent_size = LedgerEntryData::from_xdr_base64(&persistent_xdr, Limits::none())
        .unwrap()
        .to_xdr(Limits::none())
        .unwrap()
        .len();
    let temporary_size = LedgerEntryData::from_xdr_base64(&temporary_xdr, Limits::none())
        .unwrap()
        .to_xdr(Limits::none())
        .unwrap()
        .len();

    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(move |request: &wiremock::Request| {
            let body: Value = serde_json::from_slice(&request.body).unwrap();
            match body["method"].as_str().unwrap() {
                "getLatestLedger" => ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "result": { "sequence": 5_400_000, "protocolVersion": 22 }
                })),
                "getLedgers" => ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "result": { "ledgers": [] }
                })),
                "getLedgerEntries" => {
                    let keys = body["params"]["keys"].as_array().unwrap();
                    let entries: Vec<Value> = keys
                        .iter()
                        .filter_map(|key| {
                            let key = key.as_str().unwrap();
                            if key == instance_key {
                                Some(serde_json::json!({
                                    "key": key,
                                    "xdr": instance_xdr,
                                    "liveUntilLedgerSeq": 5_600_000,
                                    "lastModifiedLedgerSeq": 5_390_000
                                }))
                            } else if key == temporary_key {
                                Some(serde_json::json!({
                                    "key": key,
                                    "xdr": temporary_xdr,
                                    "liveUntilLedgerSeq": 5_450_000,
                                    "lastModifiedLedgerSeq": 5_390_000
                                }))
                            } else if key == persistent_key {
                                Some(serde_json::json!({
                                    "key": key,
                                    "xdr": persistent_xdr,
                                    "liveUntilLedgerSeq": 5_450_000,
                                    "lastModifiedLedgerSeq": 5_390_000
                                }))
                            } else {
                                None
                            }
                        })
                        .collect();
                    ResponseTemplate::new(200).set_body_json(serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": 1,
                        "result": { "entries": entries }
                    }))
                }
                other => panic!("Unexpected RPC method: {other}"),
            }
        })
        .mount(&server)
        .await;

    (server, instance_size, persistent_size, temporary_size)
}

#[tokio::test]
async fn budget_reports_contract_persistent_and_temporary_storage() {
    let contract_bytes = [21u8; 32];
    let contract_id = stellar_strkey::Contract(contract_bytes).to_string();
    let (server, instance_size, persistent_size, temporary_size) = mock_rpc(contract_bytes).await;

    let output = Command::cargo_bin("sentinel")
        .unwrap()
        .args([
            "budget",
            &contract_id,
            "--horizon-days",
            "30",
            "--fee-rate-stroops-per-byte-ledger",
            "2",
            "--temporary-key",
            "CACHE",
            "--key",
            "ADMIN",
            "--rpc-url",
            &server.uri(),
            "--format",
            "json",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["horizon_days"], 30);
    assert_eq!(report["entries"].as_array().unwrap().len(), 3);
    assert_eq!(report["entries"][0]["key_identifier"], "instance");
    assert_eq!(report["entries"][1]["key_identifier"], "persistent:ADMIN");
    assert_eq!(report["entries"][2]["key_identifier"], "temporary:CACHE");
    let horizon_ledgers = 30u128 * 17_280;
    assert_eq!(
        report["total_stroops"].as_u64().unwrap() as u128,
        (instance_size as u128 + persistent_size as u128 + temporary_size as u128)
            * horizon_ledgers
            * 2
    );
    assert_ne!(report["total_xlm"].as_str().unwrap(), "0.0000000");
}

#[tokio::test]
async fn budget_handles_contracts_without_persistent_keys() {
    let contract_bytes = [22u8; 32];
    let contract_id = stellar_strkey::Contract(contract_bytes).to_string();
    let (server, _, _, _) = mock_rpc(contract_bytes).await;
    let output = Command::cargo_bin("sentinel")
        .unwrap()
        .args([
            "budget",
            &contract_id,
            "--fee-rate-stroops-per-byte-ledger",
            "1",
            "--rpc-url",
            &server.uri(),
            "--format",
            "json",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["entries"].as_array().unwrap().len(), 1);
    assert_eq!(report["entries"][0]["key_identifier"], "instance");
}
