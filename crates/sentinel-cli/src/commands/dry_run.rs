use std::time::Duration;

use sentinel_core::{
    adapters::{parse_contract_id, parse_key_expr, validate_rpc_url, SorobanRpcClient},
    RetryPolicy,
};
use serde::Serialize;

use crate::{args::DryRunArgs, commands::resolve_rpc_url};

pub const EXIT_INVALID_ARGS: i32 = 10;
pub const EXIT_RPC_UNREACHABLE: i32 = 20;

#[derive(Serialize)]
struct CheckResult<'a> {
    check: &'a str,
    status: &'a str,
    detail: String,
}

pub async fn run_dry_run(args: DryRunArgs) -> i32 {
    let mut checks = Vec::new();
    if let Err(error) = parse_contract_id(&args.contract_id) {
        checks.push(CheckResult {
            check: "contract_id",
            status: "failed",
            detail: error.to_string(),
        });
    } else {
        checks.push(CheckResult {
            check: "contract_id",
            status: "passed",
            detail: "valid contract strkey and checksum".into(),
        });
    }
    for expression in &args.key_expr {
        match parse_key_expr(expression) {
            Ok(_) => checks.push(CheckResult {
                check: "key_expr",
                status: "passed",
                detail: "valid key expression".into(),
            }),
            Err(error) => checks.push(CheckResult {
                check: "key_expr",
                status: "failed",
                detail: error.to_string(),
            }),
        }
    }
    let rpc_url = match resolve_rpc_url("testnet", args.rpc_url.as_deref()) {
        Ok(url) => url,
        Err(error) => {
            checks.push(CheckResult {
                check: "rpc_url",
                status: "failed",
                detail: error,
            });
            print_checks(&checks);
            return EXIT_INVALID_ARGS;
        }
    };
    if let Err(error) = validate_rpc_url(&rpc_url) {
        checks.push(CheckResult {
            check: "rpc_url",
            status: "failed",
            detail: error.to_string(),
        });
    }
    if checks.iter().any(|check| check.status == "failed") {
        print_checks(&checks);
        return EXIT_INVALID_ARGS;
    }

    let rpc = SorobanRpcClient::new_with_timeout(rpc_url, Duration::from_secs(5))
        .with_retry_policy(RetryPolicy {
            max_retries: 1,
            ..RetryPolicy::default()
        });
    match rpc.get_health().await {
        Ok(health) if health.status.eq_ignore_ascii_case("healthy") && health.latest_ledger > 0 => {
            checks.push(CheckResult {
                check: "rpc_health",
                status: "passed",
                detail: format!("{} at ledger {}", health.status, health.latest_ledger),
            });
            print_checks(&checks);
            0
        }
        Ok(health) => {
            checks.push(CheckResult {
                check: "rpc_health",
                status: "failed",
                detail: format!("{} at ledger {}", health.status, health.latest_ledger),
            });
            print_checks(&checks);
            EXIT_RPC_UNREACHABLE
        }
        Err(_) => {
            checks.push(CheckResult {
                check: "rpc_health",
                status: "failed",
                detail: "getHealth request failed".into(),
            });
            print_checks(&checks);
            EXIT_RPC_UNREACHABLE
        }
    }
}

fn print_checks(checks: &[CheckResult<'_>]) {
    let result = serde_json::json!({ "status": if checks.iter().any(|item| item.status == "failed") { "failed" } else { "passed" }, "checks": checks });
    println!(
        "{}",
        serde_json::to_string_pretty(&result).expect("check result serializes")
    );
}
