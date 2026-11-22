use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::json;

static NEXT: AtomicUsize = AtomicUsize::new(0);

fn report_path(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "sentinel-{name}-{}-{}.json",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}

fn report(contract_id: &str, remaining: i64, status: &str) -> serde_json::Value {
    json!({
        "contract_id": contract_id, "network": "testnet", "latest_ledger": 200,
        "ledger_velocity_seconds": 5.0,
        "summary": {"total_entries": 1, "ok": 0, "warn": 1, "critical": 0, "archived": 0},
        "entries": [{"identifier": "persistent:Balance", "entry_type": "contract_data", "durability": "persistent", "size_bytes": 10, "live_until_ledger": 300, "remaining_ledgers": remaining, "estimated_remaining_seconds": 5.0, "estimated_remaining_days": 0.0, "status": status}]
    })
}

#[test]
fn diff_reports_outputs_deltas_and_fails_on_regression() {
    let old = report_path("old");
    let new = report_path("new");
    fs::write(
        &old,
        serde_json::to_vec(&report(
            "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM",
            100,
            "OK",
        ))
        .unwrap(),
    )
    .unwrap();
    fs::write(
        &new,
        serde_json::to_vec(&report(
            "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM",
            50,
            "WARN",
        ))
        .unwrap(),
    )
    .unwrap();
    Command::cargo_bin("sentinel")
        .unwrap()
        .args(["diff-reports"])
        .arg(&old)
        .arg(&new)
        .args(["--format", "json", "--fail-on-regression"])
        .assert()
        .code(2)
        .stdout(predicate::str::contains("\"remaining_ledgers_delta\": -50"))
        .stdout(predicate::str::contains("\"regressions\": 1"));
    fs::remove_file(old).unwrap();
    fs::remove_file(new).unwrap();
}

#[test]
fn diff_reports_rejects_mismatched_contracts() {
    let old = report_path("old-contract");
    let new = report_path("new-contract");
    fs::write(
        &old,
        serde_json::to_vec(&report(
            "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM",
            100,
            "OK",
        ))
        .unwrap(),
    )
    .unwrap();
    fs::write(
        &new,
        serde_json::to_vec(&report(
            "CBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB",
            50,
            "WARN",
        ))
        .unwrap(),
    )
    .unwrap();
    Command::cargo_bin("sentinel")
        .unwrap()
        .args(["diff-reports"])
        .arg(&old)
        .arg(&new)
        .assert()
        .code(10)
        .stderr(predicate::str::contains("contract IDs do not match"));
    fs::remove_file(old).unwrap();
    fs::remove_file(new).unwrap();
}
