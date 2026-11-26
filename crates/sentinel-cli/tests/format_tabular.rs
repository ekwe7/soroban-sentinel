use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(feature = "parquet")]
use arrow_array::{Array, Int64Array, StringArray, TimestampMicrosecondArray};
use assert_cmd::Command;
#[cfg(feature = "parquet")]
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;

use predicates::prelude::*;
use stellar_xdr::curr::{
    ContractDataDurability, ContractDataEntry, ContractExecutable, ExtensionPoint, Hash,
    LedgerEntryData, Limits, ScAddress, ScContractInstance, ScSymbol, ScVal, WriteXdr,
};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use sentinel_core::adapters::{derive_contract_instance_key, derive_persistent_symbol_key};

const EXPECTED_HEADER: [&str; 9] = [
    "timestamp",
    "network",
    "contract_id",
    "key_xdr",
    "durability",
    "byte_size",
    "live_until_ledger",
    "remaining_ledgers",
    "status",
];

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

fn make_persistent_symbol_xdr(contract_id: [u8; 32], symbol: &str) -> String {
    let entry_data = LedgerEntryData::ContractData(ContractDataEntry {
        ext: ExtensionPoint::V0,
        contract: ScAddress::Contract(Hash(contract_id)),
        key: ScVal::Symbol(ScSymbol(symbol.try_into().unwrap())),
        durability: ContractDataDurability::Persistent,
        val: ScVal::U32(42),
    });
    entry_data.to_xdr_base64(Limits::none()).unwrap()
}

fn assert_iso8601_utc(value: &str) {
    assert_eq!(value.len(), 20, "timestamp should be YYYY-MM-DDTHH:MM:SSZ");
    assert!(value.ends_with('Z'));
    let bytes = value.as_bytes();
    for &index in &[4, 7, 10, 13, 16] {
        assert!(matches!(bytes[index], b'-' | b'T' | b':'));
    }
    for (index, byte) in bytes.iter().enumerate() {
        if !matches!(index, 4 | 7 | 10 | 13 | 16 | 19) {
            assert!(byte.is_ascii_digit(), "non-digit at index {}", index);
        }
    }
}

fn parse_csv(stdout: &[u8]) -> (Vec<String>, Vec<csv::StringRecord>) {
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(true)
        .from_reader(stdout);
    let headers = reader
        .headers()
        .unwrap()
        .iter()
        .map(str::to_string)
        .collect::<Vec<_>>();
    let records = reader
        .records()
        .map(|record| record.unwrap())
        .collect::<Vec<_>>();
    (headers, records)
}

fn unique_path(prefix: &str) -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "sentinel-{}-{}-{}.csv",
        prefix,
        std::process::id(),
        nanos
    ))
}

#[cfg(feature = "parquet")]
fn unique_parquet_path(prefix: &str) -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "sentinel-{}-{}-{}.parquet",
        prefix,
        std::process::id(),
        nanos
    ))
}

#[cfg(feature = "parquet")]
fn parquet_batches(bytes: &[u8]) -> Vec<arrow_array::RecordBatch> {
    // The in-memory Parquet reader requires a `ChunkReader`; use a temporary
    // file so no extra test dependency is needed.
    let path = unique_path("parquet-read");
    fs::write(&path, bytes).unwrap();
    let builder = ParquetRecordBatchReaderBuilder::try_new(fs::File::open(&path).unwrap()).unwrap();
    assert_eq!(builder.schema().fields().len(), EXPECTED_HEADER.len());
    for (field, expected) in builder.schema().fields().iter().zip(EXPECTED_HEADER.iter()) {
        assert_eq!(field.name(), expected);
    }
    let batches: Vec<_> = builder
        .build()
        .unwrap()
        .map(|batch| batch.unwrap())
        .collect();
    fs::remove_file(&path).unwrap();
    batches
}

#[tokio::test]
async fn test_csv_stdout_parses_with_exact_header() {
    let mock_server = MockServer::start().await;
    let contract_id = [7u8; 32];
    let contract_strkey = stellar_strkey::Contract(contract_id).to_string();
    let instance_key = derive_contract_instance_key(&contract_id).unwrap();
    let mock_instance_key = instance_key.clone();
    let instance_xdr = make_instance_xdr(contract_id);
    let live_until = 5_600_000u32;

    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(move |req: &wiremock::Request| {
            let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
            match body["method"].as_str().unwrap() {
                "getLatestLedger" => ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "result": { "sequence": 5_400_000, "protocolVersion": 22 }
                })),
                "getLedgerEntries" => ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "result": {
                        "latestLedger": 5_400_000,
                        "entries": [{
                            "key": mock_instance_key,
                            "xdr": instance_xdr,
                            "liveUntilLedgerSeq": live_until,
                            "lastModifiedLedgerSeq": 5_390_000
                        }]
                    }
                })),
                other => panic!("Unexpected method: {}", other),
            }
        })
        .mount(&mock_server)
        .await;

    let output = Command::cargo_bin("sentinel")
        .unwrap()
        .args([
            "scan",
            &contract_strkey,
            "--rpc-url",
            &mock_server.uri(),
            "--format",
            "csv",
        ])
        .assert()
        .code(0)
        .get_output()
        .stdout
        .clone();

    assert!(
        !output.contains(&0x1b),
        "CSV output must not contain ANSI escapes"
    );

    let (headers, records) = parse_csv(&output);
    assert_eq!(headers, EXPECTED_HEADER);
    assert_eq!(records.len(), 1);

    let row = &records[0];
    assert_eq!(row.len(), EXPECTED_HEADER.len());
    assert_iso8601_utc(&row[0]);
    assert_eq!(&row[1], "testnet");
    assert_eq!(&row[2], &contract_strkey);
    assert_eq!(&row[3], &instance_key);
    assert_eq!(&row[4], "instance");
    assert!(row[5].parse::<u64>().unwrap() > 0);
    assert_eq!(&row[6], "5600000");
    assert_eq!(&row[7], "200000");
    assert_eq!(&row[8], "ok");
}

#[tokio::test]
async fn test_csv_output_file_and_persistent_key_xdr() {
    let mock_server = MockServer::start().await;
    let contract_id = [9u8; 32];
    let contract_strkey = stellar_strkey::Contract(contract_id).to_string();
    let instance_key = derive_contract_instance_key(&contract_id).unwrap();
    let persistent_key = derive_persistent_symbol_key(&contract_id, "Balance").unwrap();
    let mock_instance_key = instance_key.clone();
    let mock_persistent_key = persistent_key.clone();
    let instance_xdr = make_instance_xdr(contract_id);
    let persistent_xdr = make_persistent_symbol_xdr(contract_id, "Balance");
    let output_path = unique_path("csv-file");

    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(move |req: &wiremock::Request| {
            let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
            match body["method"].as_str().unwrap() {
                "getLatestLedger" => ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "result": { "sequence": 5_400_000, "protocolVersion": 22 }
                })),
                "getLedgerEntries" => ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "result": {
                        "latestLedger": 5_400_000,
                        "entries": [
                            {
                                "key": mock_instance_key,
                                "xdr": instance_xdr,
                                "liveUntilLedgerSeq": 5_600_000,
                                "lastModifiedLedgerSeq": 5_390_000
                            },
                            {
                                "key": mock_persistent_key,
                                "xdr": persistent_xdr,
                                "liveUntilLedgerSeq": 5_560_000,
                                "lastModifiedLedgerSeq": 5_390_000
                            }
                        ]
                    }
                })),
                "getLedgers" => ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "result": { "ledgers": [] }
                })),
                other => panic!("Unexpected method: {}", other),
            }
        })
        .mount(&mock_server)
        .await;

    // Pre-seed the destination to verify the documented overwrite behavior.
    fs::write(&output_path, "stale contents").unwrap();

    let output = Command::cargo_bin("sentinel")
        .unwrap()
        .args([
            "scan",
            &contract_strkey,
            "--rpc-url",
            &mock_server.uri(),
            "--format",
            "csv",
            "--key",
            "Balance",
            "--output",
            output_path.to_str().unwrap(),
        ])
        .assert()
        .code(0)
        .get_output()
        .stdout
        .clone();

    assert!(output.is_empty(), "--output must keep stdout empty");

    let file_contents = fs::read(&output_path).unwrap();
    let (headers, records) = parse_csv(&file_contents);
    assert_eq!(headers, EXPECTED_HEADER);
    assert_eq!(records.len(), 2);
    assert_eq!(&records[0][3], &instance_key);

    let persistent_row = records
        .iter()
        .find(|row| row[3] == persistent_key)
        .expect("derived persistent key XDR should be exported");
    assert_eq!(&persistent_row[4], "persistent");
    assert_eq!(&persistent_row[6], "5560000");
    assert_eq!(&persistent_row[7], "160000");
    assert_eq!(&persistent_row[8], "ok");

    fs::remove_file(&output_path).unwrap();
}

#[test]
fn test_csv_rejects_baseline_with_exit_code_10() {
    let contract_strkey = stellar_strkey::Contract([3u8; 32]).to_string();

    Command::cargo_bin("sentinel")
        .unwrap()
        .args([
            "scan",
            &contract_strkey,
            "--rpc-url",
            "http://127.0.0.1:1",
            "--format",
            "csv",
            "--baseline",
            "baseline.json",
        ])
        .assert()
        .code(10)
        .stderr(predicate::str::contains(
            "--baseline is not supported with --format csv",
        ));
}

#[test]
fn test_csv_rejects_unwritable_output_with_exit_code_10() {
    let contract_strkey = stellar_strkey::Contract([4u8; 32]).to_string();
    let missing_parent =
        std::env::temp_dir().join(format!("sentinel-missing-dir-{}", std::process::id()));
    let output_path = missing_parent.join("report.csv");
    let _ = fs::remove_dir_all(&missing_parent);

    Command::cargo_bin("sentinel")
        .unwrap()
        .args([
            "scan",
            &contract_strkey,
            "--rpc-url",
            "http://127.0.0.1:1",
            "--format",
            "csv",
            "--output",
            output_path.to_str().unwrap(),
        ])
        .assert()
        .code(10)
        .stderr(predicate::str::contains("--output parent directory"));
}

#[test]
fn test_help_advertises_csv_parquet_and_output() {
    Command::cargo_bin("sentinel")
        .unwrap()
        .args(["scan", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("csv"))
        .stdout(predicate::str::contains("parquet"))
        .stdout(predicate::str::contains("--output <PATH>"));
}

#[test]
fn test_parquet_rejects_baseline_with_exit_code_10() {
    let contract_strkey = stellar_strkey::Contract([3u8; 32]).to_string();

    Command::cargo_bin("sentinel")
        .unwrap()
        .args([
            "scan",
            &contract_strkey,
            "--rpc-url",
            "http://127.0.0.1:1",
            "--format",
            "parquet",
            "--baseline",
            "baseline.json",
        ])
        .assert()
        .code(10);
}

#[cfg(not(feature = "parquet"))]
#[test]
fn test_parquet_without_feature_fails_with_exit_code_10() {
    let contract_strkey = stellar_strkey::Contract([5u8; 32]).to_string();

    Command::cargo_bin("sentinel")
        .unwrap()
        .args([
            "scan",
            &contract_strkey,
            "--rpc-url",
            "http://127.0.0.1:1",
            "--format",
            "parquet",
        ])
        .assert()
        .code(10)
        .stderr(predicate::str::contains(
            "this binary was built without Parquet support; rebuild with --features parquet",
        ));
}

#[cfg(feature = "parquet")]
#[tokio::test]
async fn test_parquet_stdout_matches_csv_row_values() {
    let mock_server = MockServer::start().await;
    let contract_id = [7u8; 32];
    let contract_strkey = stellar_strkey::Contract(contract_id).to_string();
    let instance_key = derive_contract_instance_key(&contract_id).unwrap();
    let mock_instance_key = instance_key.clone();
    let instance_xdr = make_instance_xdr(contract_id);

    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(move |req: &wiremock::Request| {
            let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
            match body["method"].as_str().unwrap() {
                "getLatestLedger" => ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "result": { "sequence": 5_400_000, "protocolVersion": 22 }
                })),
                "getLedgerEntries" => ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "result": {
                        "latestLedger": 5_400_000,
                        "entries": [{
                            "key": mock_instance_key,
                            "xdr": instance_xdr,
                            "liveUntilLedgerSeq": 5_600_000,
                            "lastModifiedLedgerSeq": 5_390_000
                        }]
                    }
                })),
                "getLedgers" => ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "result": { "ledgers": [] }
                })),
                other => panic!("Unexpected method: {}", other),
            }
        })
        .mount(&mock_server)
        .await;

    let mut command = Command::cargo_bin("sentinel").unwrap();
    let parquet_output = command
        .args([
            "scan",
            &contract_strkey,
            "--rpc-url",
            &mock_server.uri(),
            "--format",
            "parquet",
        ])
        .assert()
        .code(0)
        .get_output()
        .stdout
        .clone();

    assert!(parquet_output.starts_with(b"PAR1"));
    assert!(parquet_output.ends_with(b"PAR1"));

    let batches = parquet_batches(&parquet_output);
    assert_eq!(batches.len(), 1);
    let batch = &batches[0];
    assert_eq!(batch.num_rows(), 1);
    assert_eq!(batch.num_columns(), 9);

    let timestamp = batch
        .column(0)
        .as_any()
        .downcast_ref::<TimestampMicrosecondArray>()
        .unwrap();
    assert!(!timestamp.is_null(0));
    assert!(timestamp.value(0) > 0);
    assert_eq!(timestamp.timezone(), Some("UTC"));

    let text = |index| -> String {
        batch
            .column(index)
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap()
            .value(0)
            .to_string()
    };
    assert_eq!(text(1), "testnet");
    assert_eq!(text(2), contract_strkey);
    assert_eq!(text(3), instance_key);
    assert_eq!(text(4), "instance");
    assert_eq!(text(8), "ok");

    let number = |index| -> i64 {
        batch
            .column(index)
            .as_any()
            .downcast_ref::<Int64Array>()
            .unwrap()
            .value(0)
    };
    assert!(number(5) > 0);
    assert_eq!(number(6), 5_600_000);
    assert_eq!(number(7), 200_000);

    // The same scan through the CSV formatter must yield the same values.
    let csv_output = Command::cargo_bin("sentinel")
        .unwrap()
        .args([
            "scan",
            &contract_strkey,
            "--rpc-url",
            &mock_server.uri(),
            "--format",
            "csv",
        ])
        .assert()
        .code(0)
        .get_output()
        .stdout
        .clone();
    let (_, records) = parse_csv(&csv_output);
    assert_eq!(records.len(), 1);
    let row = &records[0];
    assert_eq!(row[1], text(1));
    assert_eq!(row[2], text(2));
    assert_eq!(row[3], text(3));
    assert_eq!(row[4], text(4));
    assert_eq!(row[5].parse::<i64>().unwrap(), number(5));
    assert_eq!(row[6].parse::<i64>().unwrap(), number(6));
    assert_eq!(row[7].parse::<i64>().unwrap(), number(7));
    assert_eq!(row[8], text(8));
    assert_iso8601_utc(&row[0]);
}

#[cfg(feature = "parquet")]
#[tokio::test]
async fn test_parquet_output_file_keeps_stdout_empty() {
    let mock_server = MockServer::start().await;
    let contract_id = [11u8; 32];
    let contract_strkey = stellar_strkey::Contract(contract_id).to_string();
    let instance_key = derive_contract_instance_key(&contract_id).unwrap();
    let mock_instance_key = instance_key.clone();
    let instance_xdr = make_instance_xdr(contract_id);
    let output_path = unique_parquet_path("parquet-file");

    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(move |req: &wiremock::Request| {
            let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
            match body["method"].as_str().unwrap() {
                "getLatestLedger" => ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "result": { "sequence": 5_400_000, "protocolVersion": 22 }
                })),
                "getLedgerEntries" => ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "result": {
                        "latestLedger": 5_400_000,
                        "entries": [{
                            "key": mock_instance_key,
                            "xdr": instance_xdr,
                            "liveUntilLedgerSeq": 5_600_000,
                            "lastModifiedLedgerSeq": 5_390_000
                        }]
                    }
                })),
                "getLedgers" => ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "result": { "ledgers": [] }
                })),
                other => panic!("Unexpected method: {}", other),
            }
        })
        .mount(&mock_server)
        .await;

    let stdout = Command::cargo_bin("sentinel")
        .unwrap()
        .args([
            "scan",
            &contract_strkey,
            "--rpc-url",
            &mock_server.uri(),
            "--format",
            "parquet",
            "--output",
            output_path.to_str().unwrap(),
        ])
        .assert()
        .code(0)
        .get_output()
        .stdout
        .clone();
    assert!(stdout.is_empty(), "--output must keep stdout empty");

    let bytes = fs::read(&output_path).unwrap();
    let batches = parquet_batches(&bytes);
    assert_eq!(batches[0].num_rows(), 1);
    fs::remove_file(&output_path).unwrap();
}
