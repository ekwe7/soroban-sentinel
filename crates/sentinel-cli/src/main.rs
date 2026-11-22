
#![forbid(unsafe_code)]

use clap::Parser;
use sentinel_cli::args::{Cli, Commands};
use sentinel_cli::commands::{
    run_bundle,
    run_check,
    run_scan,
    EXIT_INVALID_ARGS,
};
use sentinel_cli::commands::{run_bundle, run_check, run_cost_matrix, run_scan, EXIT_INVALID_ARGS};
use sentinel_cli::commands;
use sentinel_cli::telemetry::{self, TelemetryConfig};


#[tokio::main]
async fn main() {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(err) => {
            if err.use_stderr() {
                let _ = err.print();
                std::process::exit(commands::EXIT_INVALID_ARGS);
            } else {
                err.exit();
            }
        }
    };

    let telemetry = telemetry::init(&TelemetryConfig::resolve(cli.otlp_endpoint.as_deref()));

    let exit_code = match cli.command {
        Commands::Check(args) => commands::run_check(args).await,
        Commands::CostMatrix(args) => commands::run_cost_matrix(args).await,
        Commands::Scan(args) => commands::run_scan(args).await,
        Commands::Bundle(args) => commands::run_bundle(args).await,
        Commands::InstallService(args) => commands::run_install_service(args),
        Commands::Restore(args) => commands::run_restore(args).await,
        Commands::Init(args) => commands::run_init(args),
        Commands::KeyEncode(args) => commands::run_key_encode(args),
        Commands::KeyDecode(args) => commands::run_key_decode(args),
        Commands::DiffReports(args) => commands::run_diff_reports(args),
        Commands::Forecast(args) => commands::run_forecast(args).await,
    };

    // Flush buffered spans/metrics before exiting, since `process::exit`
    // bypasses destructors.
    telemetry.shutdown();
    std::process::exit(exit_code);
}
