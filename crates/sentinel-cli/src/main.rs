#![forbid(unsafe_code)]

use clap::Parser;
use sentinel_cli::args::{Cli, Commands};
use sentinel_cli::commands::{doctor::run_doctor, run_scan, watch::run_watch, EXIT_INVALID_ARGS};

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
        Commands::Scan(args) => run_scan(args).await,
        Commands::Doctor(args) => run_doctor(args).await,
        Commands::Watch(args) => run_watch(args).await,
    };

    std::process::exit(exit_code);
}
