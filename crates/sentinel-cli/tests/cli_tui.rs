use assert_cmd::Command;
use predicates::prelude::*;
use stellar_strkey::Contract;

fn contract_id() -> String {
    Contract([7u8; 32]).to_string()
}

#[cfg(not(feature = "tui"))]
#[test]
fn tui_flag_without_feature_exits_10_with_clear_message() {
    Command::cargo_bin("sentinel")
        .unwrap()
        .args([
            "scan",
            &contract_id(),
            "--tui",
            "--rpc-url",
            "http://127.0.0.1:1",
        ])
        .assert()
        .code(10)
        .stderr(
            predicate::str::contains("--tui")
                .and(predicate::str::contains("tui"))
                .and(predicate::str::contains("feature")),
        );
}

#[cfg(feature = "tui")]
#[test]
fn tui_flag_with_non_tty_stdout_exits_10_with_clear_message() {
    Command::cargo_bin("sentinel")
        .unwrap()
        .args([
            "scan",
            &contract_id(),
            "--tui",
            "--rpc-url",
            "http://127.0.0.1:1",
        ])
        .assert()
        .code(10)
        .stderr(predicate::str::contains(
            "--tui requires an interactive TTY",
        ));
}
