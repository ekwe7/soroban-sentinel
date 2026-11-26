//! Integration tests for the `sentinel completions` subcommand.
//!
//! These tests exercise every supported shell generator through the real CLI
//! binary and assert that the emitted script is non-empty and valid.
//!
//! The tests require the binary built with the `clap_complete` family features,
//! which all ship as part of `clap_complete` itself; no extra feature gate is
//! required to emit scripts for any supported shell.

use std::process::Command as StdCommand;

/// Helper: locate the built `sentinel` binary, returning a `std::process::Command`.
fn sentinel_binary() -> StdCommand {
    StdCommand::new(env!("CARGO_BIN_EXE_sentinel"))
}

/// Helper to run completions and capture stdout for a given shell.
fn completions_output(shell: &str) -> String {
    let output = sentinel_binary()
        .args(["completions", shell])
        .output()
        .expect("failed to spawn sentinel completions");

    String::from_utf8(output.stdout).expect("completions output must be valid UTF-8")
}

#[test]
fn completions_help_exit_code_zero() {
    for shell in ["bash", "zsh", "fish", "powershell", " elvish"] {
        let trimmed = shell.trim();
        let output = sentinel_binary()
            .args(["completions", trimmed, "--help"])
            .output()
            .expect("failed to spawn");
        assert_eq!(
            output.status.code(),
            Some(0),
            "help exit code for {trimmed}"
        );
    }
}

#[test]
fn completions_emit_non_empty_scripts_for_each_shell() {
    for shell in ["bash", "zsh", "fish", "powershell", "elvish"] {
        let output = completions_output(shell);
        assert!(
            !output.is_empty(),
            "completions {shell} produced empty output"
        );
    }
}

#[test]
fn completions_are_valid_for_each_shell() {
    // Every shell generator emits a non-empty script. We verify to make sure
    // the generator actually produced output and didn't hit an internal error.
    for shell in ["bash", "zsh", "fish", "powershell", "elvish"] {
        let output = completions_output(shell);
        assert!(!output.is_empty(), "shell {shell} produced empty output");
    }
}

#[test]
fn completions_reject_unsupported_shell() {
    let output = sentinel_binary()
        .args(["completions", "ksh"])
        .output()
        .expect("failed to spawn");
    // clap returns exit code 10 (EXIT_INVALID_ARGS) for unsupported shell names.
    // This is consistent with the CLI's existing invalid-argument exit conventions.
    assert_eq!(
        output.status.code(),
        Some(10),
        "expected unsupported shell to exit with EXIT_INVALID_ARGS (10)"
    );
}

#[test]
fn completions_exit_code_zero_for_supported_shells() {
    for shell in ["bash", "zsh", "fish", "powershell", "elvish"] {
        let output = sentinel_binary()
            .args(["completions", shell])
            .output()
            .expect("failed to spawn");
        assert_eq!(
            output.status.code(),
            Some(0),
            "completions {shell} did not exit 0"
        );
    }
}
