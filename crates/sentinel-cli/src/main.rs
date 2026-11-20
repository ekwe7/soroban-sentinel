#![forbid(unsafe_code)]

use clap::Parser;
use sentinel_cli::args::{Cli, Commands};
use sentinel_cli::commands::{lint_manifest, run_scan, schema_command, EXIT_INVALID_ARGS};

#[tokio::main]
async fn main() {
    let cli = match Cli::try_parse() {
        Ok(c) => c,
        Err(err) => {
            if err.use_stderr() {
                let _ = err.print();
                std::process::exit(EXIT_INVALID_ARGS);
            } else {
                err.exit();
            }
        }
    };

    let exit_code = match cli.command {
        Commands::Scan(args) => run_scan(*args).await,
        Commands::LintManifest(args) => {
            let (code, summary) = lint_manifest(&args.manifest);
            println!("{}", summary);
            code
        }
        Commands::Schema(args) => schema_command(&args.manifest),
    };

    std::process::exit(exit_code);
}
