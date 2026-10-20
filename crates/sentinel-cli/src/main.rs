use clap::Parser;
use sentinel_cli::args::{Cli, Commands};

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Scan(args) => {
            println!("Scanning contract: {}", args.contract_id);
            println!("Target network: {}", args.network);
            if let Some(ref rpc) = args.rpc_url {
                println!("RPC URL: {}", rpc);
            }
            println!("Output format: {}", args.format);
            println!("Threshold: {} ledgers", args.threshold);
            if !args.key.is_empty() {
                println!("Persistent keys: {:?}", args.key);
            }
            if !args.key_xdr.is_empty() {
                println!("Persistent XDR keys: {:?}", args.key_xdr);
            }
        }
    }
}
