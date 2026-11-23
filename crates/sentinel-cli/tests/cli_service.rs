use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use assert_cmd::Command;
use predicates::prelude::*;

static NEXT: AtomicUsize = AtomicUsize::new(0);

fn fixture_manifest() -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "sentinel-service-{}-{}.toml",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::write(
        &path,
        "[[contracts]]\nid = 'CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM'\n",
    )
    .unwrap();
    path
}

#[test]
fn systemd_file_contains_hardening_and_user() {
    let manifest = fixture_manifest();
    let output_dir = std::env::temp_dir().join(format!(
        "sentinel-systemd-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));

    let mut command = Command::cargo_bin("sentinel").unwrap();
    command
        .args(["install-service", "--systemd", "--manifest"])
        .arg(&manifest)
        .arg("--user")
        .arg("sentinel")
        .arg("--output-dir")
        .arg(&output_dir)
        .assert()
        .success();

    let target = output_dir.join("sentinel.service");
    let content = fs::read_to_string(&target).unwrap();
    assert!(content.contains("NoNewPrivileges=true"));
    assert!(content.contains("ProtectSystem=full"));
    assert!(content.contains("Restart=on-failure"));
    assert!(content.contains("User=sentinel"));
    assert!(content.contains("watch --config"));

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o644
        );
    }

    fs::remove_dir_all(output_dir).unwrap();
    fs::remove_file(manifest).unwrap();
}

#[test]
fn launchd_file_contains_required_keys() {
    let manifest = fixture_manifest();
    let output_dir = std::env::temp_dir().join(format!(
        "sentinel-launchd-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));

    let mut command = Command::cargo_bin("sentinel").unwrap();
    command
        .args(["install-service", "--launchd", "--manifest"])
        .arg(&manifest)
        .arg("--output-dir")
        .arg(&output_dir)
        .assert()
        .success();

    let target = output_dir.join("com.soroban.sentinel.plist");
    let content = fs::read_to_string(&target).unwrap();
    assert!(content.contains("<key>KeepAlive</key>"));
    assert!(content.contains("<key>StandardOutPath</key>"));
    assert!(content.contains("<key>EnvironmentVariables</key>"));

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    fs::remove_dir_all(output_dir).unwrap();
    fs::remove_file(manifest).unwrap();
}

#[test]
fn mutually_exclusive_types_and_missing_manifest_are_rejected() {
    let mut conflict = Command::cargo_bin("sentinel").unwrap();
    conflict
        .args([
            "install-service",
            "--systemd",
            "--launchd",
            "--manifest",
            "sentinel.toml",
        ])
        .assert()
        .code(10);

    let missing = std::env::temp_dir().join(format!(
        "sentinel-missing-service-{}-{}.toml",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let mut command = Command::cargo_bin("sentinel").unwrap();
    command
        .args(["install-service", "--systemd", "--manifest"])
        .arg(&missing)
        .assert()
        .code(10)
        .stderr(predicate::str::contains("No such file or directory"));
}

#[test]
fn stdout_prints_valid_systemd_when_no_output_directory_is_used() {
    let manifest = fixture_manifest();
    let mut command = Command::cargo_bin("sentinel").unwrap();
    command
        .args(["install-service", "--systemd", "--manifest"])
        .arg(&manifest)
        .assert()
        .success()
        .stdout(predicate::str::contains("[Service]"))
        .stdout(predicate::str::contains("NoNewPrivileges=true"));
    fs::remove_file(manifest).unwrap();
}
