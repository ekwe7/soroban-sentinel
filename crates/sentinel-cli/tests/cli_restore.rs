use std::sync::{Arc, Mutex};

use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::Value;
use stellar_strkey::ed25519::PublicKey as AccountStrkey;
use stellar_strkey::Contract;
use stellar_xdr::curr::{
    ContractDataDurability, ContractDataEntry, ExtensionPoint, Hash, LedgerEntryData,
    LedgerFootprint, Limits, OperationBody, ReadXdr, RestoreFootprintOp, ScAddress, ScSymbol,
    ScVal, TransactionEnvelope, TransactionExt, WriteXdr,
};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use sentinel_core::adapters::derive_persistent_symbol_key;

const LATEST_LEDGER: u32 = 1_000;
const RESOURCE_FEE: u64 = 200;
const BASE_FEE: u32 = 100;

fn source_account_strkey() -> String {
    AccountStrkey([7; 32]).to_string()
}

fn persistent_entry_xdr(contract_id: [u8; 32], symbol: &str) -> String {
    LedgerEntryData::ContractData(ContractDataEntry {
        ext: ExtensionPoint::V0,
        contract: ScAddress::Contract(Hash(contract_id)),
        key: ScVal::Symbol(ScSymbol(symbol.try_into().unwrap())),
        durability: ContractDataDurability::Persistent,
        val: ScVal::U32(1),
    })
    .to_xdr_base64(Limits::none())
    .unwrap()
}

async fn mock_restore_rpc(
    archived: bool,
    expected_key: String,
    contract_id: [u8; 32],
) -> (MockServer, Arc<Mutex<Vec<String>>>) {
    let server = MockServer::start().await;
    let seen_footprints = Arc::new(Mutex::new(Vec::new()));
    let seen_for_response = Arc::clone(&seen_footprints);
    let live_entry_xdr = persistent_entry_xdr(contract_id, "ADMIN");

    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(move |request: &wiremock::Request| {
            let body: Value = serde_json::from_slice(&request.body).unwrap();
            match body["method"].as_str().unwrap() {
                "getLatestLedger" => ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "result": {
                        "sequence": LATEST_LEDGER,
                        "protocolVersion": 22
                    }
                })),
                "getLedgerEntries" => {
                    let entries = if archived {
                        Vec::new()
                    } else {
                        vec![serde_json::json!({
                            "key": expected_key,
                            "xdr": live_entry_xdr,
                            "liveUntilLedgerSeq": 5_000,
                            "lastModifiedLedgerSeq": 900
                        })]
                    };
                    ResponseTemplate::new(200).set_body_json(serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": 1,
                        "result": {
                            "latestLedger": LATEST_LEDGER,
                            "entries": entries
                        }
                    }))
                }
                "simulateTransaction" => {
                    let envelope_xdr = body["params"]["transaction"].as_str().unwrap();
                    let envelope =
                        TransactionEnvelope::from_xdr_base64(envelope_xdr, Limits::none()).unwrap();
                    let TransactionEnvelope::Tx(envelope) = envelope else {
                        panic!("expected v1 transaction envelope");
                    };
                    assert_eq!(envelope.tx.operations.len(), 1);
                    assert!(matches!(
                        envelope.tx.operations[0].body,
                        OperationBody::RestoreFootprint(RestoreFootprintOp {
                            ext: ExtensionPoint::V0
                        })
                    ));
                    let TransactionExt::V1(mut data) = envelope.tx.ext else {
                        panic!("expected Soroban transaction data");
                    };
                    assert_eq!(data.resources.footprint.read_write.len(), 1);
                    assert!(data.resources.footprint.read_only.is_empty());
                    seen_for_response.lock().unwrap().push(
                        data.resources.footprint.read_write[0]
                            .to_xdr_base64(Limits::none())
                            .unwrap(),
                    );
                    data.resources.instructions = 12;
                    let data_xdr = data.to_xdr_base64(Limits::none()).unwrap();
                    ResponseTemplate::new(200).set_body_json(serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": 1,
                        "result": {
                            "minResourceFee": "200",
                            "transactionData": data_xdr
                        }
                    }))
                }
                other => panic!("unexpected RPC method: {other}"),
            }
        })
        .mount(&server)
        .await;

    (server, seen_footprints)
}

fn assert_final_envelope(json: &Value, expected_key: &str) {
    let transaction_xdr = json["transaction_xdr_base64"].as_str().unwrap();
    let envelope = TransactionEnvelope::from_xdr_base64(transaction_xdr, Limits::none()).unwrap();
    let TransactionEnvelope::Tx(envelope) = envelope else {
        panic!("expected v1 transaction envelope");
    };
    assert!(envelope.signatures.is_empty());
    assert!(!json["signed"].as_bool().unwrap());
    assert_eq!(envelope.tx.fee, BASE_FEE + RESOURCE_FEE as u32);
    let TransactionExt::V1(data) = envelope.tx.ext else {
        panic!("expected Soroban transaction data");
    };
    assert_eq!(data.resource_fee, RESOURCE_FEE as i64);
    assert_eq!(data.resources.instructions, 12);
    assert_eq!(data.resources.footprint.read_write.len(), 1);
    assert_eq!(
        data.resources.footprint.read_write[0]
            .to_xdr_base64(Limits::none())
            .unwrap(),
        expected_key
    );
}

#[tokio::test]
async fn restore_archived_symbol_key_builds_footprint_and_fee() {
    let contract_id = [42u8; 32];
    let contract = Contract(contract_id).to_string();
    let expected_key = derive_persistent_symbol_key(&contract_id, "ADMIN").unwrap();
    let (server, seen_footprints) = mock_restore_rpc(true, expected_key.clone(), contract_id).await;

    let output = Command::cargo_bin("sentinel")
        .unwrap()
        .args([
            "restore",
            &contract,
            "--key",
            "ADMIN",
            "--source-account",
            &source_account_strkey(),
            "--rpc-url",
            &server.uri(),
            "--format",
            "json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let json: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(json["archived_keys"].as_array().unwrap().len(), 1);
    assert_eq!(json["archived_keys"][0], expected_key);
    assert_eq!(json["resource_fee_stroops"], RESOURCE_FEE);
    assert_eq!(json["base_fee_stroops"], BASE_FEE);
    assert_eq!(json["total_fee_stroops"], BASE_FEE as u64 + RESOURCE_FEE);
    assert_eq!(json["latest_ledger"], LATEST_LEDGER);
    assert_eq!(*seen_footprints.lock().unwrap(), vec![expected_key.clone()]);

    let footprint_xdr = json["footprint_xdr_base64"].as_str().unwrap();
    let footprint = LedgerFootprint::from_xdr_base64(footprint_xdr, Limits::none()).unwrap();
    assert_eq!(footprint.read_write.len(), 1);
    assert!(footprint.read_only.is_empty());
    assert_eq!(
        footprint.read_write[0]
            .to_xdr_base64(Limits::none())
            .unwrap(),
        expected_key
    );

    let command = json["stellar_commands"][0].as_str().unwrap();
    assert!(command.starts_with("stellar contract restore --id "));
    assert!(command.contains("--key ADMIN"));
    assert!(command.contains("--durability persistent"));
    assert!(command.contains("--network testnet"));
    assert!(command.contains("--source-account "));
    assert!(command.ends_with("--build-only"));

    assert_final_envelope(&json, &expected_key);
}

#[tokio::test]
async fn restore_text_output_contains_fee_xdr_and_cli_command() {
    let contract_id = [11u8; 32];
    let contract = Contract(contract_id).to_string();
    let expected_key = derive_persistent_symbol_key(&contract_id, "ADMIN").unwrap();
    let (server, _) = mock_restore_rpc(true, expected_key, contract_id).await;

    let stdout = Command::cargo_bin("sentinel")
        .unwrap()
        .args([
            "restore",
            &contract,
            "--key",
            "ADMIN",
            "--rpc-url",
            &server.uri(),
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8(stdout).unwrap();

    assert!(stdout.contains("State Restoration Plan"));
    assert!(stdout.contains("300 stroops"));
    assert!(stdout.contains("Footprint XDR (base64):"));
    assert!(stdout.contains("Unsigned transaction XDR (base64):"));
    assert!(stdout.contains("stellar contract restore --id"));
}

#[tokio::test]
async fn restore_rejects_live_key_with_exit_code_10() {
    let contract_id = [13u8; 32];
    let contract = Contract(contract_id).to_string();
    let expected_key = derive_persistent_symbol_key(&contract_id, "ADMIN").unwrap();
    let (server, _) = mock_restore_rpc(false, expected_key, contract_id).await;

    Command::cargo_bin("sentinel")
        .unwrap()
        .args([
            "restore",
            &contract,
            "--key",
            "ADMIN",
            "--rpc-url",
            &server.uri(),
        ])
        .assert()
        .code(10)
        .stderr(predicate::str::contains("No archived keys found"));
}

#[test]
fn restore_rejects_invalid_key_expression_before_rpc() {
    let contract = Contract([15u8; 32]).to_string();
    Command::cargo_bin("sentinel")
        .unwrap()
        .args([
            "restore",
            &contract,
            "--key-expr",
            "not-a-valid-expression",
            "--rpc-url",
            "http://127.0.0.1:1",
        ])
        .assert()
        .code(10)
        .stderr(predicate::str::contains("key").or(predicate::str::contains("expr")));
}

#[test]
fn restore_rejects_invalid_source_account_before_rpc() {
    let contract = Contract([16u8; 32]).to_string();
    Command::cargo_bin("sentinel")
        .unwrap()
        .args([
            "restore",
            &contract,
            "--source-account",
            "not-a-strkey",
            "--rpc-url",
            "http://127.0.0.1:1",
        ])
        .assert()
        .code(10)
        .stderr(predicate::str::contains("Invalid source account"));
}
