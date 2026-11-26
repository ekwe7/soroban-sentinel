use assert_cmd::Command;
use predicates::prelude::*;
use sentinel_core::{
    derive_contract_instance_key, derive_persistent_symbol_key, derive_temporary_symbol_key,
    parse_key_expr,
};
use serde_json::{json, Value};
use stellar_xdr::curr::{
    ContractDataDurability, ContractDataEntry, ContractExecutable, ExtensionPoint, Hash,
    LedgerEntryData, Limits, ScAddress, ScContractInstance, ScVal, WriteXdr,
};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

fn id() -> String {
    stellar_strkey::Contract([7; 32]).to_string()
}

fn instance_xdr() -> String {
    let entry_data = LedgerEntryData::ContractData(ContractDataEntry {
        ext: ExtensionPoint::V0,
        contract: ScAddress::Contract(Hash([7; 32])),
        key: ScVal::LedgerKeyContractInstance,
        durability: ContractDataDurability::Persistent,
        val: ScVal::ContractInstance(ScContractInstance {
            executable: ContractExecutable::StellarAsset,
            storage: None,
        }),
    });
    entry_data.to_xdr_base64(Limits::none()).unwrap()
}

fn data_xdr(key: ScVal, value: ScVal, durability: ContractDataDurability) -> String {
    LedgerEntryData::ContractData(ContractDataEntry {
        ext: ExtensionPoint::V0,
        contract: ScAddress::Contract(Hash([7; 32])),
        key,
        durability,
        val: value,
    })
    .to_xdr_base64(Limits::none())
    .unwrap()
}

async fn mock_rpc() -> MockServer {
    let server = MockServer::start().await;
    let entries = [
        json!({
            "key": derive_contract_instance_key(&[7; 32]).unwrap(),
            "xdr": instance_xdr(),
            "liveUntilLedgerSeq": 2_000_000,
            "lastModifiedLedgerSeq": 900,
        }),
        json!({
            "key": derive_persistent_symbol_key(&[7; 32], "OLD").unwrap(),
            "xdr": data_xdr(
                ScVal::Symbol("OLD".try_into().unwrap()),
                ScVal::U32(1),
                ContractDataDurability::Persistent,
            ),
            "liveUntilLedgerSeq": 1_172_800,
            "lastModifiedLedgerSeq": 900,
        }),
        json!({
            "key": derive_persistent_symbol_key(&[7; 32], "LIVE").unwrap(),
            "xdr": data_xdr(
                ScVal::Symbol("LIVE".try_into().unwrap()),
                parse_key_expr("vec[sym:OLD, u32:9]").unwrap(),
                ContractDataDurability::Persistent,
            ),
            "liveUntilLedgerSeq": 2_000_000,
            "lastModifiedLedgerSeq": 900,
        }),
        json!({
            "key": derive_temporary_symbol_key(&[7; 32], "CACHE").unwrap(),
            "xdr": data_xdr(
                ScVal::Symbol("CACHE".try_into().unwrap()),
                ScVal::Bytes(vec![0xaa; 4].try_into().unwrap()),
                ContractDataDurability::Temporary,
            ),
            "liveUntilLedgerSeq": 1_100_000,
            "lastModifiedLedgerSeq": 900,
        }),
    ];

    Mock::given(method("POST"))
        .respond_with(move |request: &wiremock::Request| {
            let call: Value = serde_json::from_slice(&request.body).unwrap();
            let result = match call["method"].as_str().unwrap() {
                "getLatestLedger" => json!({"sequence": 1000, "protocolVersion": 22}),
                "getLedgers" => json!({"ledgers": [
                    {"sequence": 999, "ledgerCloseTime": 1_760_000_000i64},
                    {"sequence": 1000, "ledgerCloseTime": 1_760_000_005i64}
                ]}),
                "getLedgerEntries" => {
                    let keys = call["params"]["keys"].as_array().unwrap();
                    json!({
                        "entries": entries.iter().filter(|entry| keys.contains(&entry["key"])).collect::<Vec<_>>(),
                        "latestLedger": 1000,
                    })
                }
                other => panic!("unexpected RPC method: {other}"),
            };
            ResponseTemplate::new(200)
                .set_body_json(json!({"jsonrpc": "2.0", "id": call["id"], "result": result}))
        })
        .mount(&server)
        .await;
    server
}

fn command(server: &MockServer) -> Command {
    let mut cmd = Command::cargo_bin("sentinel").unwrap();
    cmd.args([
        "prune-plan",
        &id(),
        "--rpc-url",
        &server.uri(),
        "--expire-key",
        "OLD",
        "--temp-key",
        "CACHE",
        "--key",
        "LIVE",
    ]);
    cmd
}

#[tokio::test]
async fn json_report_is_sorted_and_warns_about_referenced_entries() {
    let server = mock_rpc().await;
    let output = command(&server)
        .args(["--format", "json"])
        .assert()
        .code(0)
        .get_output()
        .stdout
        .clone();
    let report: Value = serde_json::from_slice(&output).unwrap();

    assert_eq!(report["contract_id"], id());
    assert_eq!(report["network"], "testnet");
    assert_eq!(report["latest_ledger"], 1000);
    assert_eq!(report["anchor_source"], "ledger_close");
    assert_eq!(report["anchor_timestamp"], 1_760_000_005i64);
    assert_eq!(report["warning_count"], 1);

    let entries = report["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0]["identifier"], "temporary:CACHE");
    assert_eq!(entries[0]["durability"], "temporary");
    assert_eq!(entries[0]["expiration_ledger"], 1_100_000);
    assert_eq!(entries[0]["estimated_expiration_date"], "2025-12-11");
    assert_eq!(entries[0]["safety"], "ok");
    assert_eq!(entries[1]["identifier"], "persistent:OLD");
    assert_eq!(entries[1]["expiration_ledger"], 1_172_800);
    assert_eq!(entries[1]["estimated_expiration_date"], "2025-12-16");
    assert_eq!(entries[1]["safety"], "warning");
    assert_eq!(entries[1]["referenced_by"][0], "persistent:LIVE");
    assert!(
        entries[0]["expiration_ledger"].as_u64().unwrap()
            < entries[1]["expiration_ledger"].as_u64().unwrap()
    );

    let calendar = report["calendar"].as_array().unwrap();
    assert_eq!(calendar.len(), 2);
    assert_eq!(calendar[0]["milestone"], 1);
    assert_eq!(calendar[0]["expiration_ledger"], 1_100_000);
    assert_eq!(calendar[0]["entries"][0], "temporary:CACHE");
    assert_eq!(calendar[0]["cumulative_entries"], 1);
    assert_eq!(calendar[1]["milestone"], 2);
    assert_eq!(calendar[1]["cumulative_entries"], 2);
    assert!(report["warnings"][0]
        .as_str()
        .unwrap()
        .contains("referenced by active persistent key(s)"));
}

#[tokio::test]
async fn text_output_renders_table_warning_and_calendar() {
    let server = mock_rpc().await;
    command(&server)
        .assert()
        .code(0)
        .stdout(predicate::str::contains("Prune Plan:"))
        .stdout(predicate::str::contains("Expires Ledger"))
        .stdout(predicate::str::contains("WARNING: 'persistent:OLD'"))
        .stdout(predicate::str::contains("Expiration Calendar"))
        .stdout(predicate::str::contains("2025-12-11"))
        .stdout(predicate::str::contains("temporary:CACHE"));
}

#[tokio::test]
async fn default_plan_marks_all_requested_persistent_keys() {
    let server = mock_rpc().await;
    let mut cmd = Command::cargo_bin("sentinel").unwrap();
    let output = cmd
        .args([
            "prune-plan",
            &id(),
            "--rpc-url",
            &server.uri(),
            "--key",
            "OLD",
            "--key",
            "LIVE",
            "--format",
            "json",
        ])
        .assert()
        .code(0)
        .get_output()
        .stdout
        .clone();
    let report: Value = serde_json::from_slice(&output).unwrap();
    let entries = report["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0]["identifier"], "persistent:OLD");
    assert_eq!(entries[1]["identifier"], "persistent:LIVE");
    assert_eq!(report["warning_count"], 0);
}

#[test]
fn invalid_strkey_returns_exit_code_10() {
    Command::cargo_bin("sentinel")
        .unwrap()
        .args(["prune-plan", "INVALID_STRKEY_NOT_CONTRACT"])
        .assert()
        .code(10)
        .stderr(predicate::str::contains("Invalid contract address"));
}
