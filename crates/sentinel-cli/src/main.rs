#![forbid(unsafe_code)]

use clap::Parser;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{fmt, EnvFilter};

use sentinel_cli::args::{Cli, Commands};
use sentinel_cli::commands;
use sentinel_cli::telemetry::{self, TelemetryConfig};
use std::env;
use std::process::Command;

#[tokio::main]
async fn main() {
    // Handle --version flag manually to embed git SHA and target triple
    if env::args().any(|arg| arg == "--version") {
        let git_sha = Command::new("git")
            .args(["rev-parse", "--short", "HEAD"])
            .output()
            .ok()
            .and_then(|o| {
                String::from_utf8(o.stdout)
                    .ok()
                    .map(|s| s.trim_end().to_string())
            })
            .unwrap_or_else(|| "unknown".to_string());

        let target_triple = Command::new("rustc")
            .args(["--print", "host-tuple"])
            .output()
            .ok()
            .and_then(|o| {
                String::from_utf8(o.stdout)
                    .ok()
                    .map(|s| s.trim_end().to_string())
            })
            .unwrap_or_else(|| "unknown".to_string());

        println!(
            "sentinel {} (sha: {}; target: {})",
            env!("CARGO_PKG_VERSION"),
            git_sha,
            target_triple
        );
        std::process::exit(0);
    }

    let cli = match Cli::try_parse() {
        Ok(c) => c,
        Err(err) => {
            if err.use_stderr() {
                let mut arguments = env::args().skip(1);
                while let Some(argument) = arguments.next() {
                    if argument == "--otlp-endpoint" {
                        arguments.next();
                        continue;
                    }
                    if argument.starts_with("--otlp-endpoint=") {
                        continue;
                    }
                    if argument == "check" {
                        println!("SENTINEL UNKNOWN - invalid monitoring arguments");
                        std::process::exit(3);
                    }
                    break;
                }
                let _ = err.print();
                std::process::exit(commands::EXIT_INVALID_ARGS);
            } else {
                err.exit();
            }
        }
    };

    let needs_tracer = cli.verbose > 0 || cli.quiet;
    if needs_tracer {
        install_tracer(&cli);
    }

    let telemetry = telemetry::init(&TelemetryConfig::resolve(cli.otlp_endpoint.as_deref()));

    let exit_code = match cli.command {
        Commands::ExportOpenapi(args) => commands::run_export_openapi(args),
        Commands::HealthMatrix(args) => commands::run_health_matrix(args).await,
        Commands::TopConsumers(args) => commands::run_top_consumers(args).await,
        Commands::Check(args) => commands::run_check(args).await,
        Commands::CostMatrix(args) => commands::run_cost_matrix(args).await,
        Commands::Scan(args) => commands::run_scan(*args).await,
        Commands::Bundle(args) => commands::run_bundle(args).await,
        Commands::AuditKeys(args) => commands::run_audit_keys(args).await,
        Commands::Bump(args) => commands::run_bump(args).await,
        Commands::Network(args) => commands::run_network(args).await,
        Commands::Restore(args) => commands::run_restore(args).await,
        Commands::InstallService(args) => commands::run_install_service(args),
        Commands::Init(args) => commands::run_init(args),
        Commands::GenerateDocker(args) => commands::run_generate_docker(args),
        Commands::KeyEncode(args) => commands::run_key_encode(args),
        Commands::KeyDecode(args) => commands::run_key_decode(args),
        Commands::DiffReports(args) => commands::run_diff_reports(args),
        Commands::DiffNetworks(args) => commands::run_diff_networks(args).await,
        Commands::KeyDiff(args) => commands::run_key_diff(args),
        Commands::Doctor(args) => commands::doctor::run_doctor(args).await,
        Commands::TraceRoute(args) => commands::trace_route::run_trace_route(args).await,
        Commands::Watch(args) => commands::watch::run_watch(*args).await,
        Commands::Forecast(args) => commands::run_forecast(args).await,
        Commands::Redact(args) => commands::run_redact(args),
        Commands::DryRun(args) => commands::run_dry_run(args).await,
        Commands::EstimateGas(args) => commands::run_estimate_gas(args).await,
        Commands::Trend(args) => commands::run_trend(args),
    };

    // Flush buffered spans/metrics before exiting, since `process::exit`
    // bypasses destructors.
    telemetry.shutdown();
    std::process::exit(exit_code);
}

fn install_tracer(cli: &Cli) {
    let level = if cli.quiet {
        "error"
    } else {
        match cli.verbose {
            0 => "off",
            1 => "info",
            _ => "debug",
        }
    };

    let filter = EnvFilter::try_new(level).unwrap_or_else(|_| EnvFilter::new("off"));
    let registry = tracing_subscriber::registry().with(filter);

    if cli.log_format == "json" {
        let layer = fmt::layer()
            .json()
            .with_writer(std::io::stderr)
            .with_target(false)
            .with_ansi(false);
        let _ = registry.with(layer).try_init();
    } else {
        let layer = fmt::layer()
            .with_writer(std::io::stderr)
            .with_target(false)
            .with_ansi(false);
        let _ = registry.with(layer).try_init();
    }
}
