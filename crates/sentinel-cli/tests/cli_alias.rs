//! CLI integration tests for the local contract alias registry.
//!
//! All tests are offline. A resolved target is proven by the scan getting past
//! validation and failing only on the unreachable RPC (exit code 11).

use assert_cmd::Command;
use predicates::prelude::*;
use std::path::PathBuf;

const UNREACHABLE_RPC: &str = "http://127.0.0.1:1";

fn strkey(n: u8) -> String {
    stellar_strkey::Contract([n; 32]).to_string()
}

fn alias_file(test: &str, body: &str) -> PathBuf {
    let path =
        std::env::temp_dir().join(format!("sentinel-alias-{}-{test}.toml", std::process::id()));
    std::fs::write(&path, body).unwrap();
    path
}

fn default_aliases(test: &str) -> PathBuf {
    alias_file(
        test,
        &format!(
            "[aliases]\ntreasury = \"{}\"\npool-usdc = \"{}\"\n",
            strkey(1),
            strkey(2)
        ),
    )
}

fn scan(file: &PathBuf, target: &str) -> Command {
    let mut cmd = Command::cargo_bin("sentinel").unwrap();
    cmd.env("SENTINEL_ALIASES", file)
        .args(["scan", target, "--rpc-url", UNREACHABLE_RPC]);
    cmd
}

#[test]
fn exact_alias_resolves_and_reaches_the_rpc() {
    let file = default_aliases("exact");
    scan(&file, "treasury")
        .assert()
        .code(11)
        .stderr(predicate::str::contains("Invalid contract address").not())
        .stderr(predicate::str::contains("did you mean").not());
    scan(&file, "pool-usdc").assert().code(11);
}

#[test]
fn direct_strkey_passes_through_with_aliases_configured() {
    let file = default_aliases("passthrough");
    scan(&file, &strkey(9))
        .assert()
        .code(11)
        .stderr(predicate::str::contains("Invalid contract address").not());
}

#[test]
fn typo_suggests_the_closest_alias() {
    let file = default_aliases("typo");
    scan(&file, "treasuri")
        .assert()
        .code(10)
        .stderr(predicate::str::contains("Invalid contract address"))
        .stderr(predicate::str::contains("did you mean `treasury`?"));
}

#[test]
fn unknown_alias_without_close_match_keeps_the_malformed_error() {
    let file = default_aliases("unknown");
    scan(&file, "zzzzzz")
        .assert()
        .code(10)
        .stderr(predicate::str::contains("Invalid contract address"))
        .stderr(predicate::str::contains("did you mean").not());
}

#[test]
fn alias_file_with_duplicate_target_is_rejected() {
    let id = strkey(3);
    let file = alias_file(
        "dup-target",
        &format!("[aliases]\na = \"{id}\"\nb = \"{id}\"\n"),
    );
    scan(&file, "a")
        .assert()
        .code(10)
        .stderr(predicate::str::contains("already registered"));
}

#[test]
fn alias_file_with_invalid_target_is_rejected() {
    let file = alias_file("bad-target", "[aliases]\ntreasury = \"not-a-strkey\"\n");
    scan(&file, "treasury")
        .assert()
        .code(10)
        .stderr(predicate::str::contains("not a valid contract strkey"));
}
