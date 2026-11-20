#![forbid(unsafe_code)]

use clap::Parser;
use sentinel_cli::args::{Cli, Commands};
use sentinel_cli::commands::{run_budget, run_bundle, run_check, run_scan, EXIT_INVALID_ARGS};

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
        Commands::Scan(args) => run_scan(args).await,
        Commands::Bundle(args) => run_bundle(args).await,
        Commands::Budget(args) => run_budget(args).await,
        Commands::ExportGrafana(args) => {
            sentinel_cli::commands::export_grafana::run_export_grafana(args)
        }
    };

    std::process::exit(exit_code);
}
