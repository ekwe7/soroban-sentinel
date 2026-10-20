use clap::Parser;
use sentinel_cli::args::{Cli, Commands};
use sentinel_cli::commands::run_scan;

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    let exit_code = match cli.command {
        Commands::Scan(args) => run_scan(args).await,
    };

    std::process::exit(exit_code);
}
