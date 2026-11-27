//! `sentinel completions` subcommand implementation.

use clap::CommandFactory;
use clap_complete::{generate, Shell};

use crate::args::Cli;

/// Shells supported by the completions generator.
pub const SUPPORTED_SHELLS: &[&str] = &[
    "bash",
    "elvish",
    "fish",
    "powershell",
    "zsh",
];

/// Exit code returned when an unsupported shell name is supplied.
pub const EXIT_INVALID_ARGS: i32 = 13;

/// Prints a shell completions script for the named shell to stdout.
///
/// No other output is emitted, so callers can redirect or pipe the result
/// without extra log lines or ANSI codes.
pub fn run_completions(shell: String) -> i32 {
    let shell_lower = shell.to_lowercase();
    let shell = match shell_lower.as_str() {
        "bash" => Shell::Bash,
        "elvish" => Shell::Elvish,
        "fish" => Shell::Fish,
        "powershell" => Shell::PowerShell,
        "zsh" => Shell::Zsh,
        _ => {
            eprintln!(
                "Error: unsupported shell '{}' (supported: bash, elvish, fish, powershell, zsh)",
                shell
            );
            return EXIT_INVALID_ARGS;
        }
    };

    let mut cmd = Cli::command();
    let mut out = std::io::stdout();
    generate(shell, &mut cmd, "sentinel", &mut out);
    0
}
