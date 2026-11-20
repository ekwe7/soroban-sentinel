#![forbid(unsafe_code)]

use clap::Parser;
use sentinel_cli::args::{Cli, Commands};
use sentinel_cli::commands::{
    lint_manifest, run_bundle, run_check, run_cost_matrix, run_scan, schema_command,
    EXIT_INVALID_ARGS,
};
use sentinel_cli::telemetry::{self, TelemetryConfig};

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

    let telemetry = telemetry::init(&TelemetryConfig::resolve(cli.otlp_endpoint.as_deref()));

    let exit_code = match cli.command {
        Commands::Check(args) => run_check(args).await,
        Commands::CostMatrix(args) => run_cost_matrix(args).await,
        Commands::Scan(args) => run_scan(*args).await,
        Commands::Bundle(args) => run_bundle(args).await,
        Commands::LintManifest(args) => {
            let (code, summary) = lint_manifest(&args.manifest);
            println!("{}", summary);
            code
        }
        Commands::Schema(args) => schema_command(&args.manifest),
    };

    // Flush buffered spans/metrics before exiting, since `process::exit`
    // bypasses destructors.
    telemetry.shutdown();
    std::process::exit(exit_code);
}
