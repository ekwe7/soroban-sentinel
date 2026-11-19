use std::io::{self, IsTerminal};
use std::time::{Duration, Instant};

use comfy_table::{Cell, Color, Table};
use sentinel_core::{LedgerReader, RetryPolicy, SentinelError, SorobanRpcClient};

use crate::{args::DoctorArgs, commands::resolve_rpc_url};

const MIN_SOROBAN_PROTOCOL: u32 = 20;

pub async fn run_doctor(args: DoctorArgs) -> i32 {
    let mut table = Table::new();
    table.set_header(["Check", "Result", "Details"]);
    let mut passed = true;
    let mut row = |name, ok, detail: String| {
        passed &= ok;
        let mut status = Cell::new(if ok { "PASS" } else { "FAIL" });
        if io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none() {
            status = status.fg(if ok { Color::Green } else { Color::Red });
        }
        table.add_row(vec![Cell::new(name), status, Cell::new(detail)]);
    };

    let expected = match args.network.to_ascii_lowercase().as_str() {
        "mainnet" => Some("Public Global Stellar Network ; September 2015"),
        "testnet" => Some("Test SDF Network ; September 2015"),
        "futurenet" => Some("Test SDF Future Network ; October 2022"),
        _ => None,
    };
    if let (Some(expected), Ok(url)) = (
        expected,
        resolve_rpc_url(&args.network, args.rpc_url.as_deref()),
    ) {
        let rpc = SorobanRpcClient::new_with_timeout(url, Duration::from_secs(5))
            .with_retry_policy(RetryPolicy {
                max_retries: 1,
                ..RetryPolicy::default()
            });
        let started = Instant::now();
        let health = rpc.get_health().await;
        row(
            "RPC connectivity",
            !matches!(health, Err(SentinelError::RpcConnectionTimeout(_))),
            format!("{} ms round trip", started.elapsed().as_millis()),
        );
        match health {
            Ok(health) => row(
                "RPC health",
                health.status == "healthy" && health.latest_ledger > 0,
                format!("{} at ledger {}", health.status, health.latest_ledger),
            ),
            Err(_) => row("RPC health", false, "No valid getHealth response".into()),
        }
        let (ledger, network) = tokio::join!(rpc.get_latest_ledger(), rpc.get_network());
        match &ledger {
            Ok(ledger) => row(
                "Latest ledger",
                ledger.sequence > 0,
                ledger.sequence.to_string(),
            ),
            Err(_) => row(
                "Latest ledger",
                false,
                "No valid getLatestLedger response".into(),
            ),
        }
        match &network {
            Ok(network) => row(
                "Network passphrase",
                network.passphrase == expected,
                format!("Expected {} network", args.network),
            ),
            Err(_) => row(
                "Network passphrase",
                false,
                "No valid getNetwork response".into(),
            ),
        }
        match (ledger, network) {
            (Ok(ledger), Ok(network)) => row(
                "Protocol",
                ledger.protocol_version >= MIN_SOROBAN_PROTOCOL
                    && network.protocol_version >= MIN_SOROBAN_PROTOCOL,
                format!(
                    "Ledger protocol {}, network protocol {} (minimum {})",
                    ledger.protocol_version, network.protocol_version, MIN_SOROBAN_PROTOCOL
                ),
            ),
            _ => row("Protocol", false, "Protocol version unavailable".into()),
        }
    } else {
        row(
            "RPC configuration",
            false,
            "Expected an HTTP(S) RPC URL and mainnet, testnet, or futurenet".into(),
        );
    }

    println!("{table}");
    i32::from(!passed)
}
