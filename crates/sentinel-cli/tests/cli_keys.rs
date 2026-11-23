use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::Value;

#[test]
fn key_encode_and_decode_round_trip_as_json() {
    let encoded = Command::cargo_bin("sentinel")
        .unwrap()
        .args([
            "key-encode",
            "sym:Balance",
            "--contract",
            "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM",
            "--format",
            "json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let encoded: Value = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(encoded["kind"], "LedgerKey");
    let xdr = encoded["xdr"].as_str().unwrap();
    let decoded = Command::cargo_bin("sentinel")
        .unwrap()
        .args(["key-decode", xdr, "--format", "json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let decoded: Value = serde_json::from_slice(&decoded).unwrap();
    assert_eq!(decoded["kind"], "LedgerKey");
    assert_eq!(decoded["representation"], "Balance");
}

#[test]
fn key_decode_rejects_invalid_xdr() {
    Command::cargo_bin("sentinel")
        .unwrap()
        .args(["key-decode", "not-base64"])
        .assert()
        .code(10)
        .stderr(predicate::str::contains("invalid LedgerKey or ScVal XDR"));
}
