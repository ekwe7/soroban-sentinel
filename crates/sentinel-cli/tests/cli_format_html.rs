use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use assert_cmd::Command;
use predicates::prelude::*;
use stellar_xdr::curr::{
    ContractDataDurability, ContractDataEntry, ContractExecutable, ExtensionPoint, Hash,
    LedgerEntryData, Limits, ScAddress, ScContractInstance, ScSymbol, ScVal, WriteXdr,
};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use sentinel_core::adapters::{derive_contract_instance_key, derive_persistent_symbol_key};

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

async fn mock_scan_server() -> (MockServer, String) {
    let mock_server = MockServer::start().await;
    let contract_id = [23u8; 32];
    let contract_strkey = stellar_strkey::Contract(contract_id).to_string();
    let instance_key = derive_contract_instance_key(&contract_id).unwrap();
    let persistent_key = derive_persistent_symbol_key(&contract_id, "Balance").unwrap();
    let instance_xdr = make_instance_xdr(contract_id);
    let persistent_xdr = make_persistent_symbol_xdr(contract_id, "Balance");

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
                                "key": instance_key,
                                "xdr": instance_xdr,
                                "liveUntilLedgerSeq": 5_500_000,
                                "lastModifiedLedgerSeq": 5_390_000
                            },
                            {
                                "key": persistent_key,
                                "xdr": persistent_xdr,
                                "liveUntilLedgerSeq": 5_800_000,
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

    (mock_server, contract_strkey)
}

fn assert_valid_html5(html: &str) {
    assert!(html.starts_with("<!DOCTYPE html>"));
    let head_end = html.find("</head>").expect("head must be closed");
    let head = &html[..head_end];
    assert!(head.contains("<html lang=\"en\">"));
    assert!(head.contains("<meta charset=\"utf-8\">"));
    assert!(head.contains("<style>"), "CSS must be embedded in head");
    assert!(head.contains("</style>"), "style element must be closed");
    assert!(html.contains("<body>") && html.contains("</body>"));
    assert!(html.trim_end().ends_with("</html>"));
    assert!(!html.contains("<script"), "report must not contain scripts");
    assert!(
        !html.contains("<link"),
        "report must not link external assets"
    );
    assert!(!html.contains("http://") && !html.contains("https://"));
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

fn unique_path() -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "sentinel-html-{}-{}.html",
        std::process::id(),
        nanos
    ))
}

#[tokio::test]
async fn test_cli_scan_html_output_is_self_contained_and_has_target_rows() {
    let (mock_server, contract_strkey) = mock_scan_server().await;

    let output = Command::cargo_bin("sentinel")
        .unwrap()
        .args([
            "scan",
            &contract_strkey,
            "--rpc-url",
            &mock_server.uri(),
            "--key",
            "Balance",
            "--format",
            "html",
        ])
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();

    let html = String::from_utf8(output).unwrap();
    assert_valid_html5(&html);

    for label in [
        "Contract ID",
        "Network",
        "Scan Timestamp",
        "Current Ledger Sequence",
    ] {
        assert!(html.contains(label), "missing metadata label: {}", label);
    }
    assert!(html.contains(&contract_strkey));
    assert!(html.contains("testnet"));
    assert!(html.contains("<dd class=\"num\">5400000</dd>"));

    let timestamp = html
        .split("datetime=\"")
        .nth(1)
        .expect("timestamp must use a <time datetime> element")
        .split('"')
        .next()
        .unwrap();
    assert_iso8601_utc(timestamp);

    assert_eq!(html.matches("<table>").count(), 1);
    assert!(html.contains("<thead>") && html.contains("<tbody>"));
    assert_eq!(html.matches("<tr>").count(), 3);
    assert!(html.contains("<th scope=\"col\">Entry</th>"));
    assert!(html.contains("<td class=\"entry-id\">instance</td>"));
    assert!(html.contains("<td class=\"entry-id\">persistent:Balance</td>"));
    assert!(html.contains("<td class=\"num\">100000</td>"));
    assert!(html.contains("<td class=\"num\">400000</td>"));

    assert!(html.contains("badge-ok"));
    assert!(html.contains("badge-warn"));
    assert!(html.contains("class=\"countdown\""));
    assert!(html.contains("class=\"countdown-fill"));
    assert!(html.contains("bar-width-25"));
    assert!(html.contains("aria-valuenow=\"25\""));
    assert!(html.contains("data-ledgers=\"400000\""));
}

#[tokio::test]
async fn test_cli_scan_html_output_file_is_self_contained() {
    let (mock_server, contract_strkey) = mock_scan_server().await;
    let output_path = unique_path();

    let stdout = Command::cargo_bin("sentinel")
        .unwrap()
        .args([
            "scan",
            &contract_strkey,
            "--rpc-url",
            &mock_server.uri(),
            "--key",
            "Balance",
            "--format",
            "HTML",
            "--output",
            output_path.to_str().unwrap(),
        ])
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();
    assert!(stdout.is_empty(), "--output must keep stdout empty");

    let contents = fs::read_to_string(&output_path).unwrap();
    assert_valid_html5(&contents);
    assert!(contents.contains("<td class=\"entry-id\">instance</td>"));
    assert!(contents.contains("<td class=\"entry-id\">persistent:Balance</td>"));
    fs::remove_file(&output_path).unwrap();
}

#[test]
fn html_format_is_advertised_in_help() {
    Command::cargo_bin("sentinel")
        .unwrap()
        .args(["scan", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("html"));
}
