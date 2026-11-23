use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use assert_cmd::Command;
use predicates::prelude::*;

static NEXT: AtomicUsize = AtomicUsize::new(0);

fn path(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "sentinel-{name}-{}-{}.toml",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}

#[test]
fn init_writes_manifest_and_protects_existing_file() {
    let output = path("init");
    Command::cargo_bin("sentinel")
        .unwrap()
        .args(["init", "--non-interactive", "--output"])
        .arg(&output)
        .args([
            "--network",
            "testnet",
            "--contract",
            "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM",
            "--key",
            "COUNTER",
        ])
        .assert()
        .success();
    let contents = fs::read_to_string(&output).unwrap();
    assert!(contents.contains("network = \"testnet\""));
    assert!(contents.contains("id = \"CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM\""));
    assert!(contents.contains("keys = [\"COUNTER\"]"));
    Command::cargo_bin("sentinel")
        .unwrap()
        .args(["init", "--non-interactive", "--output"])
        .arg(&output)
        .args([
            "--contract",
            "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM",
        ])
        .assert()
        .code(10)
        .stderr(predicate::str::contains("--force"));
    fs::remove_file(output).unwrap();
}

#[test]
fn init_rejects_invalid_contract() {
    Command::cargo_bin("sentinel")
        .unwrap()
        .args(["init", "--non-interactive", "--contract", "not-a-contract"])
        .assert()
        .code(10)
        .stderr(predicate::str::contains("invalid contract ID"));
}
