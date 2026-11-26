use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use assert_cmd::Command;
use predicates::prelude::*;
use sentinel_cli::config::load_manifest;
use serde_json::json;
use stellar_xdr::curr::{Limits, ScSymbol, ScVal, ScVec, WriteXdr};

const CONTRACT: &str = "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM";

static NEXT: AtomicUsize = AtomicUsize::new(0);

fn temp_path(name: &str, extension: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "sentinel-export-{name}-{}-{}.{extension}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}

fn xdr(value: ScVal) -> String {
    value.to_xdr_base64(Limits::none()).unwrap()
}

fn symbol(name: &str) -> ScVal {
    ScVal::Symbol(ScSymbol(name.try_into().unwrap()))
}

fn balance_vec_xdr() -> String {
    xdr(ScVal::Vec(Some(ScVec(
        vec![symbol("Balance"), ScVal::U32(42)].try_into().unwrap(),
    ))))
}

fn entry(identifier: &str, durability: &str) -> serde_json::Value {
    json!({
        "identifier": identifier, "entry_type": "contract_data", "durability": durability,
        "size_bytes": 10, "live_until_ledger": 300, "remaining_ledgers": 100,
        "estimated_remaining_seconds": 500.0, "estimated_remaining_days": 0.0, "status": "OK"
    })
}

fn report(entries: Vec<serde_json::Value>) -> serde_json::Value {
    json!({
        "contract_id": CONTRACT, "network": "testnet", "latest_ledger": 200,
        "ledger_velocity_seconds": 5.0,
        "summary": {"total_entries": entries.len(), "ok": entries.len(), "warn": 0, "critical": 0, "archived": 0},
        "entries": entries
    })
}

/// Report mixing built-in entries, duplicates, a symbol given as raw XDR,
/// a typed expression, an opaque XDR key, and a temporary key.
fn mixed_entries() -> Vec<serde_json::Value> {
    vec![
        entry("instance", "instance"),
        entry("code_wasm", "code"),
        entry("persistent:COUNTER", "persistent"),
        entry("persistent:ADMIN", "persistent"),
        entry(
            &format!("persistent_xdr:{}", xdr(symbol("COUNTER"))),
            "persistent",
        ),
        entry(
            &format!("persistent_xdr:{}", xdr(ScVal::U32(7))),
            "persistent",
        ),
        entry("persistent:vec[sym:Balance, u32:42]", "persistent"),
        entry("persistent:ADMIN", "persistent"),
        entry("temporary:Nonce", "temporary"),
    ]
}

fn write_report(entries: Vec<serde_json::Value>) -> PathBuf {
    let path = temp_path("report", "json");
    fs::write(&path, serde_json::to_vec(&report(entries)).unwrap()).unwrap();
    path
}

fn export(input: &PathBuf, format: &str) -> Command {
    let mut command = Command::cargo_bin("sentinel").unwrap();
    command
        .arg("export-keys")
        .arg("--input")
        .arg(input)
        .args(["--format", format]);
    command
}

fn stdout(command: &mut Command) -> String {
    String::from_utf8(command.assert().success().get_output().stdout.clone()).unwrap()
}

#[test]
fn export_keys_toml_is_a_valid_deduplicated_manifest_snippet() {
    let input = write_report(mixed_entries());
    let output = stdout(&mut export(&input, "toml"));

    let mut expected_xdr = vec![xdr(ScVal::U32(7)), balance_vec_xdr()];
    expected_xdr.sort();
    let expected = format!(
        "[[contracts]]\nid = \"{CONTRACT}\"\nkeys = [\"ADMIN\", \"COUNTER\"]\nkey_xdr = [\"{}\", \"{}\"]\n",
        expected_xdr[0], expected_xdr[1]
    );
    assert_eq!(output, expected);

    // The snippet must load through the real sentinel.toml manifest loader.
    let manifest = temp_path("manifest", "toml");
    fs::write(&manifest, &output).unwrap();
    let scan = load_manifest(&manifest, None).unwrap();
    assert_eq!(scan.contracts.len(), 1);
    let request = &scan.contracts[0].request;
    assert_eq!(request.contract_id_strkey, CONTRACT);
    assert_eq!(request.persistent_symbols, ["ADMIN", "COUNTER"]);
    assert_eq!(request.persistent_xdr_keys, expected_xdr);
}

#[test]
fn export_keys_toml_notes_omitted_temporary_keys() {
    let input = write_report(mixed_entries());
    export(&input, "toml")
        .assert()
        .success()
        .stdout(predicate::str::contains("Nonce").not())
        .stderr(predicate::str::contains("1 temporary key(s) omitted"));
}

#[test]
fn export_keys_json_lists_persistent_then_temporary_keys() {
    let input = write_report(mixed_entries());
    let json: serde_json::Value =
        serde_json::from_str(&stdout(&mut export(&input, "json"))).unwrap();

    assert_eq!(json["contract_id"], CONTRACT);
    assert_eq!(json["network"], "testnet");
    let mut opaque = [xdr(ScVal::U32(7)), balance_vec_xdr()];
    opaque.sort();
    assert_eq!(
        json["keys"],
        json!([
            {"durability": "persistent", "symbol": "ADMIN", "key_xdr": xdr(symbol("ADMIN"))},
            {"durability": "persistent", "symbol": "COUNTER", "key_xdr": xdr(symbol("COUNTER"))},
            {"durability": "persistent", "key_xdr": opaque[0]},
            {"durability": "persistent", "key_xdr": opaque[1]},
            {"durability": "temporary", "symbol": "Nonce", "key_xdr": xdr(symbol("Nonce"))},
        ])
    );
}

#[test]
fn export_keys_output_is_independent_of_report_entry_order() {
    let forward = write_report(mixed_entries());
    let mut reversed_entries = mixed_entries();
    reversed_entries.reverse();
    let reversed = write_report(reversed_entries);

    for format in ["toml", "json"] {
        assert_eq!(
            stdout(&mut export(&forward, format)),
            stdout(&mut export(&reversed, format))
        );
    }
}

#[test]
fn export_keys_report_without_user_keys_exports_empty_lists() {
    let input = write_report(vec![
        entry("instance", "instance"),
        entry("code_wasm", "code"),
    ]);
    let output = stdout(&mut export(&input, "toml"));
    assert!(output.contains("keys = []"));
    assert!(output.contains("key_xdr = []"));
}

#[test]
fn export_keys_rejects_missing_and_malformed_input() {
    let missing = temp_path("missing", "json");
    export(&missing, "toml")
        .assert()
        .code(10)
        .stderr(predicate::str::contains(missing.display().to_string()));

    let malformed = temp_path("malformed", "json");
    fs::write(&malformed, "{ not json").unwrap();
    export(&malformed, "json")
        .assert()
        .code(10)
        .stderr(predicate::str::contains("invalid TTL report"));

    let wrong_shape = temp_path("shape", "json");
    fs::write(&wrong_shape, r#"{"contract_id": "C"}"#).unwrap();
    export(&wrong_shape, "json")
        .assert()
        .code(10)
        .stderr(predicate::str::contains("invalid TTL report"));
}

#[test]
fn export_keys_rejects_undecodable_or_unknown_identifiers() {
    let bad_xdr = write_report(vec![entry("persistent_xdr:%%%", "persistent")]);
    export(&bad_xdr, "toml")
        .assert()
        .code(10)
        .stderr(predicate::str::contains("entry 'persistent_xdr:%%%'"));

    let unknown = write_report(vec![entry("mystery:COUNTER", "persistent")]);
    export(&unknown, "json")
        .assert()
        .code(10)
        .stderr(predicate::str::contains(
            "unrecognized entry identifier 'mystery:COUNTER'",
        ));
}
