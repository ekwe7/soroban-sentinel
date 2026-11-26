use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use assert_cmd::Command;
use predicates::prelude::*;

static NEXT: AtomicUsize = AtomicUsize::new(0);

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "sentinel-test-workflow-{name}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn test_generate_workflow_default_options() {
    let dir = temp_dir("default");
    Command::cargo_bin("sentinel")
        .unwrap()
        .args(["generate-github-workflow", "--out-dir"])
        .arg(&dir)
        .assert()
        .success()
        .stdout(predicate::str::contains("Successfully generated"));

    let workflow_path = dir
        .join(".github")
        .join("workflows")
        .join("sentinel-ttl-audit.yml");
    assert!(workflow_path.exists());

    let contents = fs::read_to_string(&workflow_path).unwrap();
    assert!(contents.contains("name: Sentinel TTL Audit"));
    assert!(contents.contains("cron: '0 0 * * *'"));
    assert!(contents.contains("workflow_dispatch:"));
    assert!(contents.contains("actions/checkout@v4"));
    assert!(contents.contains("dtolnay/rust-toolchain@stable"));
    assert!(contents.contains("cargo install soroban-sentinel --locked"));
    assert!(contents.contains("SENTINEL_CONTRACT_ID: ${{ vars.SENTINEL_CONTRACT_ID }}"));
    assert!(contents.contains(
        "sentinel scan \"$SENTINEL_CONTRACT_ID\" --network mainnet --format markdown --fail-on-risk critical"
    ));
    assert!(contents.contains("| tee -a \"$GITHUB_STEP_SUMMARY\""));
    assert!(contents.contains("      - name: Run Sentinel TTL Scan and Enforce Critical Risk"));

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_generate_workflow_custom_cron_and_network() {
    let dir = temp_dir("custom");
    Command::cargo_bin("sentinel")
        .unwrap()
        .args([
            "generate-github-workflow",
            "--out-dir",
            dir.to_str().unwrap(),
            "--cron",
            "0 12 * * 1-5",
            "--network",
            "testnet",
        ])
        .assert()
        .success();

    let workflow_path = dir
        .join(".github")
        .join("workflows")
        .join("sentinel-ttl-audit.yml");
    assert!(workflow_path.exists());

    let contents = fs::read_to_string(&workflow_path).unwrap();
    assert!(contents.contains("cron: '0 12 * * 1-5'"));
    assert!(contents.contains(
        "sentinel scan \"$SENTINEL_CONTRACT_ID\" --network testnet --format markdown --fail-on-risk critical"
    ));

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_generate_workflow_force_overwrite_protection() {
    let dir = temp_dir("overwrite");
    let workflows_dir = dir.join(".github").join("workflows");
    fs::create_dir_all(&workflows_dir).unwrap();
    let target = workflows_dir.join("sentinel-ttl-audit.yml");
    fs::write(&target, "old content").unwrap();

    // Without --force: fails with exit code 10
    Command::cargo_bin("sentinel")
        .unwrap()
        .args(["generate-github-workflow", "--out-dir"])
        .arg(&dir)
        .assert()
        .code(10)
        .stderr(predicate::str::contains("--force"));

    assert_eq!(fs::read_to_string(&target).unwrap(), "old content");

    // With --force: overwrites successfully
    Command::cargo_bin("sentinel")
        .unwrap()
        .args([
            "generate-github-workflow",
            "--out-dir",
            dir.to_str().unwrap(),
            "--force",
        ])
        .assert()
        .success();

    assert!(fs::read_to_string(&target)
        .unwrap()
        .contains("Sentinel TTL Audit"));

    let _ = fs::remove_dir_all(&dir);
}
