use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn completions_bash_prints_script_to_stdout() {
    let mut cmd = Command::cargo_bin("sentinel").unwrap();
    cmd.args(["completions", "bash"]);

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("complete -F _sentinel"))
        .stdout(predicate::str::contains("sentinel"));
}

#[test]
fn completions_zsh_prints_script_to_stdout() {
    let mut cmd = Command::cargo_bin("sentinel").unwrap();
    cmd.args(["completions", "zsh"]);

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("#compdef"));
}

#[test]
fn completions_fish_prints_script_to_stdout() {
    let mut cmd = Command::cargo_bin("sentinel").unwrap();
    cmd.args(["completions", "fish"]);

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("complete"));
}

#[test]
fn completions_powershell_prints_script_to_stdout() {
    let mut cmd = Command::cargo_bin("sentinel").unwrap();
    cmd.args(["completions", "powershell"]);

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("Register-ArgumentCompleter"));
}

#[test]
fn completions_elvish_prints_script_to_stdout() {
    let mut cmd = Command::cargo_bin("sentinel").unwrap();
    cmd.args(["completions", "elvish"]);

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("arg-completer"))
        .stdout(predicate::str::contains("sentinel"));
}

#[test]
fn completions_invalid_shell_returns_exit_13() {
    let mut cmd = Command::cargo_bin("sentinel").unwrap();
    cmd.args(["completions", "csh"]);

    cmd.assert()
        .code(10)
        .stderr(predicate::str::contains("unsupported shell"));
}

#[test]
fn completions_missing_shell_returns_exit_13() {
    let mut cmd = Command::cargo_bin("sentinel").unwrap();
    cmd.args(["completions"]);

    cmd.assert().code(10).stderr(predicate::str::contains("SHELL"));
}

#[test]
fn completions_help_lists_supported_shells() {
    let mut cmd = Command::cargo_bin("sentinel").unwrap();
    cmd.args(["completions", "--help"]);

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("bash"))
        .stdout(predicate::str::contains("zsh"))
        .stdout(predicate::str::contains("fish"))
        .stdout(predicate::str::contains("powershell"))
        .stdout(predicate::str::contains("elvish"));
}

#[test]
fn completions_is_advertised_in_main_help() {
    let mut cmd = Command::cargo_bin("sentinel").unwrap();
    cmd.arg("--help");

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("completions"));
}
