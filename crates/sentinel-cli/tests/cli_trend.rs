use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::{json, Value};

static NEXT_DIR: AtomicUsize = AtomicUsize::new(0);

const CONTRACT_A: &str = "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM";
const CONTRACT_B: &str = "CBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB2";

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "sentinel-cli-trend-{name}-{}-{}",
            std::process::id(),
            NEXT_DIR.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).expect("create temp fixture dir");
        Self(path)
    }

    fn write_json(&self, file_name: &str, value: &Value) {
        fs::write(
            self.0.join(file_name),
            serde_json::to_vec_pretty(value).expect("serialize fixture"),
        )
        .expect("write fixture");
    }

    fn path(&self) -> &PathBuf {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn entry_json(
    identifier: &str,
    entry_type: &str,
    durability: &str,
    size_bytes: usize,
    live_until_ledger: u32,
    remaining_ledgers: i64,
    status: &str,
) -> Value {
    let remaining_secs = (remaining_ledgers.max(0) as f64) * 5.0;
    let remaining_days = ((remaining_secs / 86_400.0) * 100.0).round() / 100.0;
    json!({
        "identifier": identifier,
        "entry_type": entry_type,
        "durability": durability,
        "size_bytes": size_bytes,
        "live_until_ledger": live_until_ledger,
        "remaining_ledgers": remaining_ledgers,
        "estimated_remaining_seconds": remaining_secs,
        "estimated_remaining_days": remaining_days,
        "status": status
    })
}

fn scan_report(
    contract_id: &str,
    network: &str,
    latest_ledger: u32,
    timestamp: Option<&str>,
    entries: Vec<Value>,
) -> Value {
    let mut report = json!({
        "contract_id": contract_id,
        "network": network,
        "latest_ledger": latest_ledger,
        "ledger_velocity_seconds": 5.0,
        "summary": {
            "total_entries": entries.len(),
            "ok": entries.len(),
            "warn": 0,
            "critical": 0,
            "archived": 0
        },
        "entries": entries
    });
    if let Some(ts) = timestamp {
        report["timestamp"] = json!(ts);
    }
    report
}

#[test]
fn trend_tabular_output_on_historical_fixture_directory() {
    let dir = TempDir::new("tabular");

    // Write 4 reports with intentionally non-chronological filenames to verify sorting.
    dir.write_json(
        "z_day1.json",
        &scan_report(
            CONTRACT_A,
            "testnet",
            5_000_000,
            Some("2026-01-01T00:00:00Z"),
            vec![
                entry_json(
                    "instance",
                    "contract_instance",
                    "instance",
                    342,
                    5_293_760,
                    293_760,
                    "ok",
                ),
                entry_json(
                    "persistent:Admin",
                    "contract_data",
                    "persistent",
                    64,
                    5_350_000,
                    350_000,
                    "ok",
                ),
            ],
        ),
    );
    dir.write_json(
        "m_day2.json",
        &scan_report(
            CONTRACT_A,
            "testnet",
            5_017_280,
            Some("2026-01-02T00:00:00Z"),
            vec![
                entry_json(
                    "instance",
                    "contract_instance",
                    "instance",
                    342,
                    5_293_760,
                    276_480,
                    "ok",
                ),
                entry_json(
                    "persistent:Admin",
                    "contract_data",
                    "persistent",
                    64,
                    5_350_000,
                    332_400,
                    "ok",
                ),
            ],
        ),
    );
    dir.write_json(
        "b_day3.json",
        &scan_report(
            CONTRACT_A,
            "testnet",
            5_034_560,
            Some("2026-01-03T00:00:00Z"),
            vec![
                entry_json(
                    "instance",
                    "contract_instance",
                    "instance",
                    342,
                    5_293_760,
                    259_200,
                    "ok",
                ),
                entry_json(
                    "persistent:Admin",
                    "contract_data",
                    "persistent",
                    64,
                    5_350_000,
                    315_600,
                    "ok",
                ),
            ],
        ),
    );
    dir.write_json(
        "a_day4.json",
        &scan_report(
            CONTRACT_A,
            "testnet",
            5_051_840,
            Some("2026-01-04T00:00:00Z"),
            vec![
                entry_json(
                    "instance",
                    "contract_instance",
                    "instance",
                    342,
                    5_293_760,
                    241_920,
                    "ok",
                ),
                entry_json(
                    "persistent:Admin",
                    "contract_data",
                    "persistent",
                    64,
                    5_350_000,
                    297_800,
                    "ok",
                ),
            ],
        ),
    );

    Command::cargo_bin("sentinel")
        .unwrap()
        .args(["trend", "--input-dir"])
        .arg(dir.path())
        .assert()
        .code(0)
        .stdout(predicate::str::contains(CONTRACT_A))
        .stdout(predicate::str::contains("Snapshots: 4"))
        .stdout(predicate::str::contains("5,000,000 -> 5,051,840"))
        .stdout(predicate::str::contains("Entry"))
        .stdout(predicate::str::contains("Slope (L/day)"))
        .stdout(predicate::str::contains("Warn Crossing (95% CI)"))
        .stdout(predicate::str::contains("Critical Crossing (95% CI)"))
        .stdout(predicate::str::contains("instance"))
        .stdout(predicate::str::contains("persistent:Admin"))
        .stdout(predicate::str::contains("-17,280.0"))
        .stdout(predicate::str::contains("2026-01-11"))
        .stdout(predicate::str::contains("2026-01-16"))
        .stdout(predicate::str::contains("Summary: 2 entries analyzed."));
}

#[test]
fn trend_json_output_verifies_slope_projections_and_confidence_intervals() {
    let dir = TempDir::new("json");

    let days = [
        (
            "snap_1.json",
            5_000_000u32,
            "2026-01-01T00:00:00Z",
            293_760i64,
            300_000i64,
        ),
        (
            "snap_2.json",
            5_017_280u32,
            "2026-01-02T00:00:00Z",
            276_480i64,
            282_300i64,
        ),
        (
            "snap_3.json",
            5_034_560u32,
            "2026-01-03T00:00:00Z",
            259_200i64,
            265_700i64,
        ),
        (
            "snap_4.json",
            5_051_840u32,
            "2026-01-04T00:00:00Z",
            241_920i64,
            247_900i64,
        ),
    ];
    for (file, ledger, ts, exact_rem, noisy_rem) in days {
        dir.write_json(
            file,
            &scan_report(
                CONTRACT_A,
                "mainnet",
                ledger,
                Some(ts),
                vec![
                    entry_json(
                        "instance",
                        "contract_instance",
                        "instance",
                        300,
                        5_293_760,
                        exact_rem,
                        "ok",
                    ),
                    entry_json(
                        "persistent:Pool",
                        "contract_data",
                        "persistent",
                        128,
                        5_300_000,
                        noisy_rem,
                        "ok",
                    ),
                ],
            ),
        );
    }

    let output = Command::cargo_bin("sentinel")
        .unwrap()
        .args(["trend", "--input-dir"])
        .arg(dir.path())
        .args(["--format", "json"])
        .assert()
        .code(0)
        .get_output()
        .stdout
        .clone();

    let parsed: Value = serde_json::from_slice(&output).expect("valid JSON output");
    assert_eq!(parsed["contract_id"], CONTRACT_A);
    assert_eq!(parsed["network"], "mainnet");
    assert_eq!(parsed["snapshot_count"], 4);
    assert_eq!(parsed["first_ledger"], 5_000_000);
    assert_eq!(parsed["latest_ledger"], 5_051_840);
    assert_eq!(parsed["timestamp_source"], "report");
    assert_eq!(parsed["warning_threshold"], 120_960);
    assert_eq!(parsed["critical_threshold"], 34_560);

    let entries = parsed["entries"].as_array().expect("entries array");
    assert_eq!(entries.len(), 2);

    let inst = entries
        .iter()
        .find(|e| e["identifier"] == "instance")
        .unwrap();
    assert_eq!(inst["slope_ledgers_per_day"], -17_280.0);
    assert_eq!(inst["slope_ledgers_per_second"], -0.2);
    assert_eq!(inst["r_squared"], 1.0);
    assert_eq!(inst["bump_count"], 0);
    assert_eq!(inst["warning_projection"]["state"], "projected");
    assert_eq!(inst["warning_projection"]["projected_date"], "2026-01-11");
    assert_eq!(
        inst["warning_projection"]["confidence_interval"]["level"],
        0.95
    );
    assert_eq!(
        inst["warning_projection"]["confidence_interval"]["earliest_date"],
        "2026-01-11"
    );
    assert_eq!(
        inst["warning_projection"]["confidence_interval"]["latest_date"],
        "2026-01-11"
    );
    assert_eq!(inst["critical_projection"]["state"], "projected");
    assert_eq!(inst["critical_projection"]["projected_date"], "2026-01-16");

    let pool = entries
        .iter()
        .find(|e| e["identifier"] == "persistent:Pool")
        .unwrap();
    let pool_slope = pool["slope_ledgers_per_day"].as_f64().unwrap();
    assert!(pool_slope < -17_000.0 && pool_slope > -17_700.0);
    let warn_ci = &pool["warning_projection"]["confidence_interval"];
    assert!(warn_ci["margin_days"].as_f64().unwrap() > 0.0);
    assert!(
        warn_ci["earliest_timestamp"].as_str().unwrap()
            <= pool["warning_projection"]["projected_timestamp"]
                .as_str()
                .unwrap()
    );
    assert!(
        warn_ci["latest_timestamp"].as_str().unwrap()
            >= pool["warning_projection"]["projected_timestamp"]
                .as_str()
                .unwrap()
    );
}

#[test]
fn trend_detects_discontinuous_storage_bump_between_snapshots() {
    let dir = TempDir::new("bump");

    let snapshots = [
        ("2026-01-01.json", 5_000_000u32, 5_200_000u32, 200_000i64),
        ("2026-01-02.json", 5_017_280u32, 5_200_000u32, 182_720i64),
        // TTL bumped by +300,000 ledgers before Day 3!
        ("2026-01-03.json", 5_034_560u32, 5_500_000u32, 465_440i64),
        ("2026-01-04.json", 5_051_840u32, 5_500_000u32, 448_160i64),
        ("2026-01-05.json", 5_069_120u32, 5_500_000u32, 430_880i64),
    ];

    for (file, ledger, live_until, rem) in snapshots {
        dir.write_json(
            file,
            &scan_report(
                CONTRACT_A,
                "testnet",
                ledger,
                None, // Timestamp extracted from YYYY-MM-DD filename
                vec![entry_json(
                    "persistent:Vault",
                    "contract_data",
                    "persistent",
                    128,
                    live_until,
                    rem,
                    "ok",
                )],
            ),
        );
    }

    let output = Command::cargo_bin("sentinel")
        .unwrap()
        .args(["trend", "--input-dir"])
        .arg(dir.path())
        .args(["--format", "json"])
        .assert()
        .code(0)
        .get_output()
        .stdout
        .clone();

    let parsed: Value = serde_json::from_slice(&output).expect("valid JSON");
    assert_eq!(parsed["timestamp_source"], "filename");
    let vault = &parsed["entries"][0];
    assert_eq!(vault["identifier"], "persistent:Vault");
    assert_eq!(vault["bump_count"], 1);
    assert_eq!(vault["bumps"][0]["from_ledger"], 5_017_280);
    assert_eq!(vault["bumps"][0]["to_ledger"], 5_034_560);
    assert_eq!(vault["bumps"][0]["live_until_ledger_delta"], 300_000);
    assert_eq!(vault["regression_observations"], 3);
    assert_eq!(vault["slope_ledgers_per_day"], -17_280.0);
    assert_eq!(vault["warning_projection"]["state"], "projected");
    assert_eq!(vault["warning_projection"]["projected_date"], "2026-01-22");
    assert_eq!(vault["critical_projection"]["projected_date"], "2026-01-27");

    // Also verify tabular text highlights detected TTL bumps.
    Command::cargo_bin("sentinel")
        .unwrap()
        .args(["trend", "--input-dir"])
        .arg(dir.path())
        .assert()
        .code(0)
        .stdout(predicate::str::contains("Detected TTL Bumps:"))
        .stdout(predicate::str::contains("persistent:Vault"))
        .stdout(predicate::str::contains("1 bumped"));
}

#[test]
fn trend_filters_by_contract_and_rejects_ambiguous_multi_contract_dir() {
    let dir = TempDir::new("multi-contract");

    for (idx, (ledger, ts, rem)) in [
        (5_000_000u32, "2026-02-01T00:00:00Z", 300_000i64),
        (5_017_280u32, "2026-02-02T00:00:00Z", 282_720i64),
        (5_034_560u32, "2026-02-03T00:00:00Z", 265_440i64),
    ]
    .into_iter()
    .enumerate()
    {
        dir.write_json(
            &format!("contract_a_{idx}.json"),
            &scan_report(
                CONTRACT_A,
                "testnet",
                ledger,
                Some(ts),
                vec![entry_json(
                    "instance",
                    "contract_instance",
                    "instance",
                    200,
                    5_300_000,
                    rem,
                    "ok",
                )],
            ),
        );
        dir.write_json(
            &format!("contract_b_{idx}.json"),
            &scan_report(
                CONTRACT_B,
                "testnet",
                ledger,
                Some(ts),
                vec![entry_json(
                    "code_wasm",
                    "contract_code",
                    "code",
                    10_000,
                    5_400_000,
                    rem + 100_000,
                    "ok",
                )],
            ),
        );
    }

    // Without --contract, ambiguous directory fails cleanly with exit code 10.
    Command::cargo_bin("sentinel")
        .unwrap()
        .args(["trend", "--input-dir"])
        .arg(dir.path())
        .assert()
        .code(10)
        .stderr(predicate::str::contains("multiple contracts found"))
        .stderr(predicate::str::contains("--contract"));

    // With --contract CONTRACT_B, filters exclusively to CONTRACT_B.
    let output = Command::cargo_bin("sentinel")
        .unwrap()
        .args(["trend", "--input-dir"])
        .arg(dir.path())
        .args(["--contract", CONTRACT_B, "--format", "json"])
        .assert()
        .code(0)
        .get_output()
        .stdout
        .clone();

    let parsed: Value = serde_json::from_slice(&output).expect("valid JSON");
    assert_eq!(parsed["contract_id"], CONTRACT_B);
    assert_eq!(parsed["snapshot_count"], 3);
    assert_eq!(parsed["entries"].as_array().unwrap().len(), 1);
    assert_eq!(parsed["entries"][0]["identifier"], "code_wasm");
}

#[test]
fn trend_supports_standard_scan_json_without_explicit_timestamps() {
    let dir = TempDir::new("no-ts");

    for (name, ledger, rem) in [
        ("r3.json", 5_034_560u32, 265_440i64),
        ("r1.json", 5_000_000u32, 300_000i64),
        ("r2.json", 5_017_280u32, 282_720i64),
    ] {
        dir.write_json(
            name,
            &scan_report(
                CONTRACT_A,
                "testnet",
                ledger,
                None,
                vec![entry_json(
                    "instance",
                    "contract_instance",
                    "instance",
                    256,
                    5_300_000,
                    rem,
                    "ok",
                )],
            ),
        );
    }

    let output = Command::cargo_bin("sentinel")
        .unwrap()
        .args(["trend", "--input-dir"])
        .arg(dir.path())
        .args(["--format", "json"])
        .assert()
        .code(0)
        .get_output()
        .stdout
        .clone();

    let parsed: Value = serde_json::from_slice(&output).expect("valid JSON");
    assert_eq!(parsed["timestamp_source"], "ledger_velocity");
    assert_eq!(parsed["first_ledger"], 5_000_000);
    assert_eq!(parsed["latest_ledger"], 5_034_560);
    assert_eq!(parsed["entries"][0]["slope_ledgers_per_day"], -17_280.0);
    assert_eq!(
        parsed["entries"][0]["warning_projection"]["state"],
        "projected"
    );
    assert!(parsed["entries"][0]["warning_projection"]["projected_date"].is_string());
}

#[test]
fn trend_handles_custom_thresholds_and_already_crossed_entries() {
    let dir = TempDir::new("thresholds");

    for (name, ledger, ts, rem) in [
        ("d1.json", 5_000_000u32, "2026-03-01T00:00:00Z", 180_000i64),
        ("d2.json", 5_017_280u32, "2026-03-02T00:00:00Z", 162_720i64),
        ("d3.json", 5_034_560u32, "2026-03-03T00:00:00Z", 145_440i64),
    ] {
        dir.write_json(
            name,
            &scan_report(
                CONTRACT_A,
                "testnet",
                ledger,
                Some(ts),
                vec![entry_json(
                    "instance",
                    "contract_instance",
                    "instance",
                    256,
                    5_180_000,
                    rem,
                    "warn",
                )],
            ),
        );
    }

    let output = Command::cargo_bin("sentinel")
        .unwrap()
        .args(["trend", "--input-dir"])
        .arg(dir.path())
        .args([
            "--warning-threshold",
            "200000",
            "--critical-threshold",
            "100000",
            "--format",
            "json",
        ])
        .assert()
        .code(0)
        .get_output()
        .stdout
        .clone();

    let parsed: Value = serde_json::from_slice(&output).expect("valid JSON");
    assert_eq!(parsed["warning_threshold"], 200_000);
    assert_eq!(parsed["critical_threshold"], 100_000);
    assert_eq!(
        parsed["entries"][0]["warning_projection"]["state"],
        "already_crossed"
    );
    assert_eq!(
        parsed["entries"][0]["critical_projection"]["state"],
        "projected"
    );
}

#[test]
fn trend_rejects_invalid_inputs() {
    // 1. Missing directory
    let missing = std::env::temp_dir().join("sentinel-does-not-exist-trend-dir");
    Command::cargo_bin("sentinel")
        .unwrap()
        .args(["trend", "--input-dir"])
        .arg(&missing)
        .assert()
        .code(10)
        .stderr(predicate::str::contains("does not exist"));

    // 2. Single snapshot (insufficient for regression)
    let single = TempDir::new("single");
    single.write_json(
        "only.json",
        &scan_report(
            CONTRACT_A,
            "testnet",
            5_000_000,
            Some("2026-01-01T00:00:00Z"),
            vec![entry_json(
                "instance",
                "contract_instance",
                "instance",
                200,
                5_300_000,
                300_000,
                "ok",
            )],
        ),
    );
    Command::cargo_bin("sentinel")
        .unwrap()
        .args(["trend", "--input-dir"])
        .arg(single.path())
        .assert()
        .code(10)
        .stderr(predicate::str::contains("at least 2"));

    // 3. Invalid threshold ordering
    Command::cargo_bin("sentinel")
        .unwrap()
        .args(["trend", "--input-dir"])
        .arg(single.path())
        .args([
            "--warning-threshold",
            "10000",
            "--critical-threshold",
            "20000",
        ])
        .assert()
        .code(10)
        .stderr(predicate::str::contains("Critical threshold"));

    // 4. Malformed JSON report
    let broken = TempDir::new("broken");
    fs::write(broken.path().join("bad.json"), "{not valid json").unwrap();
    Command::cargo_bin("sentinel")
        .unwrap()
        .args(["trend", "--input-dir"])
        .arg(broken.path())
        .assert()
        .code(10)
        .stderr(predicate::str::contains("invalid JSON scan report"));
}
