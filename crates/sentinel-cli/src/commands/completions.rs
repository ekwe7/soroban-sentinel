//! Shell completion generator for the `sentinel` CLI.
//!
//! Emits completion scripts for bash, zsh, fish, PowerShell, and Elvish by
//! delegating to `clap_complete`. Output is written to stdout only, with no
//! logs or ANSI codes, so it can be sourced or redirected safely.

use crate::args::{Cli, CompletionsArgs};
use clap::CommandFactory;

/// Convert a string shell name to the clap_complete aot shell.
fn to_clap_complete(shell: &str) -> Result<clap_complete::aot::Shell, String> {
    match shell {
        "bash" => Ok(clap_complete::aot::Shell::Bash),
        "zsh" => Ok(clap_complete::aot::Shell::Zsh),
        "fish" => Ok(clap_complete::aot::Shell::Fish),
        "powershell" => Ok(clap_complete::aot::Shell::PowerShell),
        "elvish" => Ok(clap_complete::aot::Shell::Elvish),
        _ => Err(format!("unsupported shell: {}", shell)),
    }
}

/// Run the completion generator and emit to stdout.
///
/// This is the single entry point used by the `completions` command. It prints
/// the completion script for the requested shell and returns success. No logs,
/// diagnostic output, or ANSI codes are emitted.
pub fn run_completions(args: CompletionsArgs) -> Result<(), String> {
    let clap_shell = to_clap_complete(&args.shell)?;
    let mut cmd = Cli::command();
    clap_complete::aot::generate(clap_shell, &mut cmd, "sentinel", &mut std::io::stdout());
    Ok(())
}
