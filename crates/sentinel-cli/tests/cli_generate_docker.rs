use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use assert_cmd::Command;
use predicates::prelude::*;

static NEXT: AtomicUsize = AtomicUsize::new(0);

fn temp_dir() -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "sentinel-generate-docker-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&path).unwrap();
    path
}

#[test]
fn generate_docker_writes_hardened_deployment_files() {
    let dir = temp_dir();
    Command::cargo_bin("sentinel")
        .unwrap()
        .args(["generate-docker", "--out-dir"])
        .arg(&dir)
        .assert()
        .success();

    let dockerfile = fs::read_to_string(dir.join("Dockerfile")).unwrap();
    assert!(dockerfile.contains("FROM rust:") && dockerfile.contains("AS builder"));
    assert!(dockerfile.contains("FROM alpine:"));
    assert!(dockerfile.contains("adduser -S -D -H -u 10001"));
    assert!(dockerfile.contains("USER 10001:10001"));

    let compose = fs::read_to_string(dir.join("docker-compose.yml")).unwrap();
    assert!(compose.contains("read_only: true"));
    assert!(compose.contains("cap_drop:"));
    assert!(compose.contains("no-new-privileges:true"));
    assert!(compose.contains("sentinel",));
    assert!(compose.contains("doctor"));
    assert!(compose.contains("SOROBAN_RPC_URL: ${SOROBAN_RPC_URL:-}"));
    assert!(compose.contains("sentinel.toml:/etc/sentinel/sentinel.toml:ro"));

    let dockerignore = fs::read_to_string(dir.join(".dockerignore")).unwrap();
    assert!(dockerignore.contains(".git"));
    assert!(dockerignore.contains("target"));
    assert!(dockerignore.contains(".env"));

    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn generate_docker_does_not_overwrite_existing_files() {
    let dir = temp_dir();
    fs::write(dir.join("Dockerfile"), "keep me").unwrap();
    Command::cargo_bin("sentinel")
        .unwrap()
        .args(["generate-docker", "--out-dir"])
        .arg(&dir)
        .assert()
        .code(10)
        .stderr(predicate::str::contains("already exists"));
    assert_eq!(
        fs::read_to_string(dir.join("Dockerfile")).unwrap(),
        "keep me"
    );
    fs::remove_dir_all(dir).unwrap();
}
