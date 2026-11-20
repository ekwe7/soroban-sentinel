use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use assert_cmd::Command;
use serde_json::Value;
use stellar_strkey::ed25519::PublicKey as AccountStrkey;
use stellar_strkey::Contract;
use stellar_xdr::curr::{
    AccountEntry, AccountEntryExt, AccountId, LedgerEntry, LedgerEntryData, LedgerEntryExt, Limits,
    PublicKey, ReadXdr, ScVal, SequenceNumber, String32, Thresholds, TransactionEnvelope,
    TransactionExt, Uint256, WriteXdr,
};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn account_id(bytes: [u8; 32]) -> AccountId {
    AccountId(PublicKey::PublicKeyTypeEd25519(Uint256(bytes)))
}

fn make_account_entry(public_key: [u8; 32], sequence: i64) -> String {
    let account = AccountEntry {
        account_id: account_id(public_key),
        balance: 100_000_000,
        seq_num: SequenceNumber(sequence),
        num_sub_entries: 0,
        inflation_dest: None,
        flags: 0,
        home_domain: String32::try_from(Vec::new()).unwrap(),
        thresholds: Thresholds([1, 1, 1, 1]),
        signers: Default::default(),
        ext: AccountEntryExt::V0,
    };
    LedgerEntry {
        last_modified_ledger_seq: 1_000,
        data: LedgerEntryData::Account(account),
        ext: LedgerEntryExt::V0,
    }
    .to_xdr_base64(Limits::none())
    .unwrap()
}

async fn mock_rpc(source_public_key: [u8; 32]) -> (MockServer, Arc<Mutex<Vec<usize>>>) {
    let server = MockServer::start().await;
    let seen_footprint_sizes = Arc::new(Mutex::new(Vec::new()));
    let seen_for_response = Arc::clone(&seen_footprint_sizes);
    let account_xdr = make_account_entry(source_public_key, 25);

    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(move |request: &wiremock::Request| {
            let body: Value = serde_json::from_slice(&request.body).unwrap();
            match body["method"].as_str().unwrap() {
                "getLatestLedger" => ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "result": {
                        "sequence": 1_000,
                        "protocolVersion": 22
                    }
                })),
                "getLedgerEntries" => {
                    let key = body["params"]["keys"][0].clone();
                    ResponseTemplate::new(200).set_body_json(serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": 1,
                        "result": {
                            "entries": [{
                                "key": key,
                                "xdr": account_xdr,
                                "lastModifiedLedgerSeq": 1_000
                            }]
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
                    let TransactionExt::V1(mut data) = envelope.tx.ext else {
                        panic!("expected Soroban transaction data");
                    };
                    seen_for_response
                        .lock()
                        .unwrap()
                        .push(data.resources.footprint.read_only.len());
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

    (server, seen_footprint_sizes)
}

fn source_account_strkey() -> String {
    AccountStrkey([7; 32]).to_string()
}

#[tokio::test]
async fn bundle_deduplicates_contracts_and_persistent_keys() {
    let source_public_key = [7; 32];
    let (server, footprint_sizes) = mock_rpc(source_public_key).await;
    let contract = Contract([42; 32]).to_string();
    let mut command = Command::cargo_bin("sentinel").unwrap();
    command.args([
        "bundle",
        "--contract",
        &contract,
        "--contract",
        &contract,
        "--key",
        "ADMIN",
        "--key",
        "ADMIN",
        "--source-account",
        &source_account_strkey(),
        "--extend-by",
        "100",
        "--rpc-url",
        &server.uri(),
    ]);

    let output = command.assert().success().get_output().stdout.clone();
    let json: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(json["latest_ledger"], 1_000);
    assert_eq!(json["extend_to_ledger"], 1_100);
    assert_eq!(json["transactions"].as_array().unwrap().len(), 1);
    assert_eq!(json["transactions"][0]["entry_count"], 2);
    assert_eq!(json["total_fee_stroops"], 300);
    assert_eq!(*footprint_sizes.lock().unwrap(), vec![2]);

    let transaction_xdr = json["transactions"][0]["transaction_xdr_base64"]
        .as_str()
        .unwrap();
    let envelope = TransactionEnvelope::from_xdr_base64(transaction_xdr, Limits::none()).unwrap();
    let TransactionEnvelope::Tx(envelope) = envelope else {
        panic!("expected v1 transaction envelope");
    };
    assert!(envelope.signatures.is_empty());
    assert_eq!(envelope.tx.seq_num.0, 26);
    assert_eq!(envelope.tx.fee, 300);
    let TransactionExt::V1(data) = envelope.tx.ext else {
        panic!("expected Soroban transaction data");
    };
    assert_eq!(data.resource_fee, 200);
    assert_eq!(data.resources.instructions, 12);
    assert_eq!(data.resources.footprint.read_only.len(), 2);
    assert!(data.resources.footprint.read_write.is_empty());
}

#[tokio::test]
async fn bundle_splits_footprints_at_forty_entries_and_sums_fees() {
    let source_public_key = [7; 32];
    let (server, footprint_sizes) = mock_rpc(source_public_key).await;
    let contracts: Vec<String> = (1..=41)
        .map(|byte| Contract([byte; 32]).to_string())
        .collect();
    let mut command = Command::cargo_bin("sentinel").unwrap();
    command.args(["bundle"]);
    for contract in &contracts {
        command.args(["--contract", contract]);
    }
    command.args([
        "--source-account",
        &source_account_strkey(),
        "--extend-by",
        "500",
        "--rpc-url",
        &server.uri(),
    ]);

    let output = command.assert().success().get_output().stdout.clone();
    let json: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(json["transactions"].as_array().unwrap().len(), 2);
    assert_eq!(json["transactions"][0]["entry_count"], 40);
    assert_eq!(json["transactions"][1]["entry_count"], 1);
    assert_eq!(json["total_fee_stroops"], 600);
    assert_eq!(*footprint_sizes.lock().unwrap(), vec![40, 1]);
}

#[tokio::test]
async fn bundle_reads_json_manifest_and_merges_command_line_keys() {
    let source_public_key = [7; 32];
    let (server, footprint_sizes) = mock_rpc(source_public_key).await;
    let contract = Contract([43; 32]).to_string();
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let manifest_path = std::env::temp_dir().join(format!(
        "sentinel-bundle-{}-{unique}.json",
        std::process::id()
    ));
    std::fs::write(
        &manifest_path,
        serde_json::json!({
            "contracts": [{
                "contract_id": contract,
                "keys": ["ADMIN"],
                "key_xdr": []
            }]
        })
        .to_string(),
    )
    .unwrap();
    let xdr_key = ScVal::U32(7).to_xdr_base64(Limits::none()).unwrap();

    let mut command = Command::cargo_bin("sentinel").unwrap();
    command.args([
        "bundle",
        "--manifest",
        manifest_path.to_str().unwrap(),
        "--key",
        "COUNTER",
        "--key-xdr",
        &xdr_key,
        "--source-account",
        &source_account_strkey(),
        "--extend-by",
        "100",
        "--rpc-url",
        &server.uri(),
    ]);

    let result = command.assert().success().get_output().stdout.clone();
    std::fs::remove_file(manifest_path).unwrap();
    let json: Value = serde_json::from_slice(&result).unwrap();
    assert_eq!(json["transactions"][0]["entry_count"], 4);
    assert_eq!(*footprint_sizes.lock().unwrap(), vec![4]);
}
