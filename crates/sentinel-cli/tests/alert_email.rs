//! Email alert notification tests for Soroban Sentinel.

use std::time::Duration;

use clap::Parser;
use sentinel_cli::Cli;

#[tokio::test]
async fn test_watch_args_parses_via_cli() {
    let cli = Cli::parse_from([
        "sentinel",
        "watch",
        "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM",
        "--network",
        "testnet",
    ]);
    match cli.command {
        sentinel_cli::Commands::Watch(wargs) => {
            assert_eq!(wargs.contract_id, "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM");
            assert_eq!(wargs.network, "testnet");
        }
        _ => panic!("expected watch command"),
    }
}

#[tokio::test]
async fn test_parse_cli_has_watch_command() {
    let cli = Cli::parse_from(["sentinel", "watch", "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM", "--network", "testnet"]);
    assert!(matches!(cli.command, sentinel_cli::Commands::Watch(_)));
}

#[tokio::test]
async fn test_watch_args_with_smtp() {
    let cli = Cli::parse_from([
        "sentinel",
        "watch",
        "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM",
        "--network",
        "testnet",
        "--smtp-server",
        "smtp.example.com",
    ]);
    match cli.command {
        sentinel_cli::Commands::Watch(wargs) => {
            assert!(wargs.smtp_server.is_some());
        }
        _ => panic!("expected watch command"),
    }
}

#[tokio::test]
async fn test_watch_args_with_webhook() {
    let cli = Cli::parse_from([
        "sentinel",
        "watch",
        "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM",
        "--network",
        "testnet",
        "--webhook-url",
        "https://example.com/webhook",
    ]);
    match cli.command {
        sentinel_cli::Commands::Watch(wargs) => {
            assert!(wargs.webhook_url.is_some());
        }
        _ => panic!("expected watch command"),
    }
}