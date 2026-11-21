//! Build script for sorbent-cli.
//!
//! Computes git short SHA and target triple, makes them available
//! via cargo:rustc-env so clap can embed them in the version string.

use std::env;
use std::process::Command;

fn main() {
    // Get short git SHA
    let git_sha = std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .unwrap_or_else(|| "unknown".to_string());

    // Get target triple
    let target_triple = env::var("TARGET").unwrap_or_else(|_| {
        Command::new("rustc")
            .args(["--print", "host-tuple"])
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .unwrap_or_else(|| "unknown".to_string())
    });

    // Set environment variable available via option_env! and std::env::var
    let full_version = format!("0.1.0 (sha: {}; target: {})", git_sha, target_triple);
    println!("cargo:rustc-env=FULL_VERSION={}", full_version);
}
