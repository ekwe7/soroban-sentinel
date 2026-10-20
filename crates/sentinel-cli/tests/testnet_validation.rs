//! Live Soroban Testnet integration tests.
//!
//! These tests connect to the real SDF public Testnet RPC at https://soroban-testnet.stellar.org.
//! They are annotated with `#[ignore]` so offline CI does not depend on external network availability.
//! Run with: `cargo test --test testnet_validation -- --ignored`

use assert_cmd::Command;
use predicates::prelude::*;
use sentinel_core::adapters::SorobanRpcClient;
use sentinel_core::domain::RiskLevel;
use sentinel_core::usecases::{ScanContractUseCase, ScanRequest};

/// Native Stellar Asset Contract (SAC) for XLM on Soroban Testnet.
/// This contract is permanently active and maintained by the Stellar network.
const TESTNET_NATIVE_SAC_CONTRACT: &str =
    "CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC";

const TESTNET_RPC_URL: &str = "https://soroban-testnet.stellar.org";

#[tokio::test]
#[ignore = "requires live internet connectivity to Soroban Testnet"]
async fn test_live_soroban_testnet_usecase_scan() {
    let client = SorobanRpcClient::new(TESTNET_RPC_URL);
    let usecase = ScanContractUseCase::new(client);

    let request = ScanRequest::new(TESTNET_NATIVE_SAC_CONTRACT).with_network("testnet");

    let report = usecase
        .execute(request)
        .await
        .expect("Live testnet scan should succeed");

    assert_eq!(report.contract_id, TESTNET_NATIVE_SAC_CONTRACT);
    assert_eq!(report.network, "testnet");
    assert!(
        report.latest_ledger > 5_000_000,
        "Expected testnet latest ledger > 5,000,000, got: {}",
        report.latest_ledger
    );
    assert_eq!(report.ledger_velocity_seconds, 5.0);

    // Verify contract instance entry is returned and has valid live_until_ledger
    assert!(
        !report.entries.is_empty(),
        "Expected at least instance entry to be present"
    );

    let instance_entry = &report.entries[0];
    assert_eq!(instance_entry.key_identifier, "instance");
    assert!(
        instance_entry.live_until_ledger > report.latest_ledger,
        "Expected instance live_until ({}) > latest_ledger ({})",
        instance_entry.live_until_ledger,
        report.latest_ledger
    );
    assert!(instance_entry.byte_size > 0);
    assert_eq!(instance_entry.status, RiskLevel::Ok);
    assert_eq!(report.overall_status, RiskLevel::Ok);
}

#[test]
#[ignore = "requires live internet connectivity to Soroban Testnet"]
fn test_live_soroban_testnet_cli_binary_table_format() {
    let mut cmd = Command::cargo_bin("sentinel").unwrap();
    cmd.args([
        "scan",
        TESTNET_NATIVE_SAC_CONTRACT,
        "--network",
        "testnet",
        "--format",
        "text",
    ]);

    cmd.assert()
        .code(0)
        .stdout(predicate::str::contains(TESTNET_NATIVE_SAC_CONTRACT))
        .stdout(predicate::str::contains("testnet"))
        .stdout(predicate::str::contains("Contract Instance"))
        .stdout(predicate::str::contains("Instance"))
        .stdout(predicate::str::contains("OK"))
        .stdout(predicate::str::contains(
            "Summary: 1 entries checked. 1 OK, 0 WARN, 0 CRITICAL/ARCHIVED.",
        ));
}

#[test]
#[ignore = "requires live internet connectivity to Soroban Testnet"]
fn test_live_soroban_testnet_cli_binary_json_format() {
    let mut cmd = Command::cargo_bin("sentinel").unwrap();
    cmd.args([
        "scan",
        TESTNET_NATIVE_SAC_CONTRACT,
        "--network",
        "testnet",
        "--format",
        "json",
    ]);

    let assert = cmd.assert().code(0);
    let stdout_bytes = assert.get_output().stdout.clone();
    let parsed: serde_json::Value =
        serde_json::from_slice(&stdout_bytes).expect("Stdout must be valid JSON");

    assert_eq!(parsed["contract_id"], TESTNET_NATIVE_SAC_CONTRACT);
    assert_eq!(parsed["network"], "testnet");
    assert!(parsed["latest_ledger"].as_u64().unwrap() > 5_000_000);
    assert_eq!(parsed["summary"]["total_entries"], 1);
    assert_eq!(parsed["summary"]["ok"], 1);

    let entries = parsed["entries"].as_array().expect("entries array");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["identifier"], "instance");
    assert_eq!(entries[0]["status"], "ok");
    assert!(entries[0]["live_until_ledger"].as_u64().unwrap() > 5_000_000);
}
