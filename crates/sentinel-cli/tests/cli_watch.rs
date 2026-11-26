use assert_cmd::Command;
use predicates::prelude::*;
use sentinel_core::adapters::{derive_contract_instance_key, derive_persistent_symbol_key};
use stellar_xdr::curr::{ContractDataDurability, ScSymbol, ScVal};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

mod common;

fn served_state(
    contract_id: [u8; 32],
    instance_live_until: u32,
    counter_live_until: u32,
) -> Vec<common::ServedEntry> {
    let counter = ScVal::Symbol(ScSymbol("COUNTER".try_into().unwrap()));
    vec![
        (
            derive_contract_instance_key(&contract_id).unwrap(),
            common::instance_entry_xdr(contract_id, None),
            instance_live_until,
        ),
        (
            derive_persistent_symbol_key(&contract_id, "COUNTER").unwrap(),
            common::data_entry_xdr(contract_id, counter, ContractDataDurability::Persistent),
            counter_live_until,
        ),
    ]
}

// Two scans against changing ledger data: full report, then only the transitions
#[ignore = "requires unification of PR #105 diff watcher with PR #133/#213 incident dispatch watcher"]
#[tokio::test]
async fn test_cli_watch_prints_transitions_between_scans() {
    let mock_server = MockServer::start().await;
    let contract_id = [100u8; 32];
    let contract_strkey = stellar_strkey::Contract(contract_id).to_string();
    common::mount_ledger_sequence(
        &mock_server,
        vec![
            // instance 200,000 left (OK); COUNTER 100,000 left (WARN)
            (5_400_000, served_state(contract_id, 5_600_000, 5_500_000)),
            // instance now 50,000 left (WARN); COUNTER bumped to 400,000 left (OK)
            (5_550_000, served_state(contract_id, 5_600_000, 5_950_000)),
        ],
    )
    .await;

    let output = Command::cargo_bin("sentinel")
        .unwrap()
        .args([
            "watch",
            &contract_strkey,
            "--rpc-url",
            &mock_server.uri(),
            "--key",
            "COUNTER",
            "--interval",
            "10",
            "--max-iterations",
            "2",
        ])
        .timeout(std::time::Duration::from_secs(60))
        .output()
        .unwrap();

    let stdout = String::from_utf8(output.stdout).unwrap();
    // Exit code reflects the last scan: the instance is now WARN.
    assert_eq!(output.status.code(), Some(1), "{stdout}");
    assert!(stdout.contains("Initial scan"));
    assert!(stdout.contains("Summary: 2 entries checked. 1 OK, 1 WARN, 0 CRITICAL/ARCHIVED."));
    assert!(stdout.contains("] Contract Instance: status OK -> WARN"));
    assert!(stdout.contains("] Persistent Key: COUNTER: status WARN -> OK"));
    assert!(stdout.contains(
        "] Persistent Key: COUNTER: live_until 5,500,000 -> 5,950,000 (+450,000 ledgers, bumped)"
    ));
    // The second scan prints transitions only, not a second table.
    assert_eq!(stdout.matches("Entry Description").count(), 1);
}

// Invalid watch flags exit 10 before any RPC request
#[ignore = "requires unification of PR #105 diff watcher with PR #133/#213 incident dispatch watcher"]
#[tokio::test]
async fn test_cli_watch_rejects_invalid_flags_before_rpc() {
    let mock_server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&mock_server)
        .await;
    let contract_strkey = stellar_strkey::Contract([101u8; 32]).to_string();

    for extra in [
        vec!["--interval", "5"],
        vec!["--interval", "86401"],
        vec!["--max-consecutive-failures", "0"],
        vec!["--max-iterations", "0"],
        vec!["--format", "markdown"],
        vec!["--rpc-timeout", "0"],
    ] {
        Command::cargo_bin("sentinel")
            .unwrap()
            .args(["watch", &contract_strkey, "--rpc-url", &mock_server.uri()])
            .args(&extra)
            .assert()
            .code(10);
    }
    mock_server.verify().await;
}

#[ignore = "requires unification of PR #105 diff watcher with PR #133/#213 incident dispatch watcher"]
#[test]
fn test_cli_watch_help() {
    Command::cargo_bin("sentinel")
        .unwrap()
        .args(["watch", "--help"])
        .assert()
        .code(0)
        .stdout(predicate::str::contains("--interval <SECS>"))
        .stdout(predicate::str::contains("--max-iterations <N>"))
        .stdout(predicate::str::contains("--max-consecutive-failures <N>"))
        .stdout(predicate::str::contains("--temp-key <SYMBOL>"))
        .stdout(predicate::str::contains("SIGTERM"));
}

// SIGTERM or Ctrl-C (SIGINT) stops a running watch cleanly with the last scan's risk code
#[ignore = "requires unification of PR #105 diff watcher with PR #133/#213 incident dispatch watcher"]
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_cli_watch_sigterm_and_sigint_exit_cleanly_with_last_status() {
    use std::io::{BufRead, BufReader};
    use std::process::Stdio;

    let mock_server = MockServer::start().await;
    let contract_id = [102u8; 32];
    let contract_strkey = stellar_strkey::Contract(contract_id).to_string();
    // COUNTER 10,000 left -> CRITICAL
    common::mount_ledger(
        &mock_server,
        5_400_000,
        served_state(contract_id, 5_600_000, 5_410_000),
    )
    .await;

    for signal in ["-TERM", "-INT"] {
        let mut child = std::process::Command::new(assert_cmd::cargo::cargo_bin("sentinel"))
            .args([
                "watch",
                &contract_strkey,
                "--rpc-url",
                &mock_server.uri(),
                "--key",
                "COUNTER",
                "--interval",
                "3600",
                "--format",
                "json",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();

        // Wait for the first NDJSON report line, then signal while the loop waits for its
        // next tick.
        let mut stdout = BufReader::new(child.stdout.take().unwrap());
        let mut first_line = String::new();
        tokio::task::block_in_place(|| stdout.read_line(&mut first_line)).unwrap();
        let report: serde_json::Value = serde_json::from_str(&first_line).unwrap();
        assert_eq!(report["type"], "report");
        assert_eq!(report["report"]["summary"]["critical"], 1);

        let killed = std::process::Command::new("kill")
            .args([signal, &child.id().to_string()])
            .status()
            .unwrap();
        assert!(killed.success());

        let status = tokio::task::block_in_place(|| child.wait()).unwrap();
        assert_eq!(
            status.code(),
            Some(2),
            "{signal}: exits with the last scan's CRITICAL code"
        );
    }
}
