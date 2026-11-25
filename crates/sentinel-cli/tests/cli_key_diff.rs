use std::fs;
use std::sync::atomic::{AtomicUsize, Ordering};

use assert_cmd::Command;
use serde_json::{json, Value};

static NEXT: AtomicUsize = AtomicUsize::new(0);

fn path(label: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "sentinel-key-diff-{label}-{}-{}.json",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}

fn report(entries: Value) -> Value {
    json!({"contract_id":"CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM","network":"testnet","latest_ledger":10,"ledger_velocity_seconds":5.0,"summary":{"total_entries":entries.as_array().unwrap().len(),"ok":0,"warn":0,"critical":0,"archived":0},"entries":entries})
}

fn entry(identifier: &str, durability: &str, size: usize) -> Value {
    json!({"identifier":identifier,"entry_type":"contract_data","durability":durability,"size_bytes":size,"live_until_ledger":100,"remaining_ledgers":90,"estimated_remaining_seconds":450.0,"estimated_remaining_days":0.01,"status":"ok"})
}

#[test]
fn key_diff_classifies_added_removed_modified_and_unchanged_keys() {
    let before = path("before");
    let after = path("after");
    fs::write(
        &before,
        report(json!([
            entry("persistent:kept", "persistent", 10),
            entry("persistent:size", "persistent", 10),
            entry("persistent:durability", "persistent", 10),
            entry("persistent:gone", "persistent", 5)
        ]))
        .to_string(),
    )
    .unwrap();
    fs::write(
        &after,
        report(json!([
            entry("persistent:kept", "persistent", 10),
            entry("persistent:size", "persistent", 11),
            entry("persistent:durability", "temporary", 10),
            entry("persistent:new", "persistent", 7)
        ]))
        .to_string(),
    )
    .unwrap();

    let output = Command::cargo_bin("sentinel")
        .unwrap()
        .args(["key-diff", "--before"])
        .arg(&before)
        .args(["--after"])
        .arg(&after)
        .args(["--format", "json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let value: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(value["summary"]["Added"], 1);
    assert_eq!(value["summary"]["Removed"], 1);
    assert_eq!(value["summary"]["ModifiedDurability"], 1);
    assert_eq!(value["summary"]["ModifiedSize"], 1);
    assert_eq!(value["summary"]["Unchanged"], 1);
    fs::remove_file(before).unwrap();
    fs::remove_file(after).unwrap();
}

#[test]
fn key_diff_reports_entries_without_recoverable_xdr_as_unresolved() {
    let before = path("unresolved-before");
    let after = path("unresolved-after");
    let code_entry = json!({"identifier":"code_wasm","entry_type":"contract_code","durability":"code","size_bytes":100,"live_until_ledger":100,"remaining_ledgers":90,"estimated_remaining_seconds":450.0,"estimated_remaining_days":0.01,"status":"ok"});
    fs::write(&before, report(json!([code_entry.clone()])).to_string()).unwrap();
    fs::write(&after, report(json!([code_entry])).to_string()).unwrap();

    let output = Command::cargo_bin("sentinel")
        .unwrap()
        .args(["key-diff", "--before"])
        .arg(&before)
        .args(["--after"])
        .arg(&after)
        .args(["--format", "json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let value: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(value["summary"]["Unresolved"], 2);
    assert_eq!(value["changes"][0]["canonical_key_xdr"], Value::Null);
    assert_eq!(
        value["unresolved"][0]["reason"],
        "contract code hash is missing from this scan report"
    );
    fs::remove_file(before).unwrap();
    fs::remove_file(after).unwrap();
}
