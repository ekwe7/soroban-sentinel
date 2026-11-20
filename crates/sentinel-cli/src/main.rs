#![forbid(unsafe_code)]

use clap::Parser;
use sentinel_cli::args::{Cli, Commands};
use sentinel_cli::commands::{run_bundle, run_check, run_scan, run_watch, EXIT_INVALID_ARGS};
use sentinel_cli::commands::{run_bundle, run_check, run_cost_matrix, run_scan, EXIT_INVALID_ARGS};

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
        Commands::Check(args) => run_check(args).await,
        Commands::CostMatrix(args) => run_cost_matrix(args).await,
        Commands::Scan(args) => run_scan(args).await,
        Commands::Watch(args) => run_watch(args).await,
        Commands::Bundle(args) => run_bundle(args).await,
    };

    std::process::exit(exit_code);
}
