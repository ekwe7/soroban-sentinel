//! One-shot, sequential fleet checks.

use sentinel_core::adapters::SorobanRpcClient;
use sentinel_core::domain::RiskLevel;
use sentinel_core::error::SentinelError;
use sentinel_core::usecases::ScanContractUseCase;
use serde::Serialize;

use crate::args::CheckArgs;
use crate::commands::{
    EXIT_CRITICAL_OR_ARCHIVED, EXIT_INVALID_ARGS, EXIT_OK, EXIT_RPC_FAILURE, EXIT_WARN,
};
use crate::config::load_manifest;
use crate::formatters::{render_text_with_options, JsonTTLReport};

#[derive(Serialize)]
struct ContractResult {
    id: String,
    name: Option<String>,
    status: Option<RiskLevel>,
    report: Option<JsonTTLReport>,
    error: Option<String>,
}

#[derive(Default, Serialize)]
struct Summary {
    total: usize,
    ok: usize,
    warn: usize,
    critical: usize,
    archived: usize,
    failed: usize,
}

impl Summary {
    fn exit_code(&self) -> i32 {
        if self.failed > 0 {
            EXIT_RPC_FAILURE
        } else if self.critical + self.archived > 0 {
            EXIT_CRITICAL_OR_ARCHIVED
        } else if self.warn > 0 {
            EXIT_WARN
        } else {
            EXIT_OK
        }
    }
}

#[derive(Serialize)]
struct FleetReport {
    contracts: Vec<ContractResult>,
    summary: Summary,
}

pub async fn run_check(args: CheckArgs) -> i32 {
    if let Some(id) = &args.contract_id {
        return run_monitoring_check(id, &args.scan).await;
    }
    let rpc_env = match std::env::var("SOROBAN_RPC_URL") {
        Ok(url) => Some(url),
        Err(std::env::VarError::NotPresent) => None,
        Err(std::env::VarError::NotUnicode(_)) => {
            eprintln!("Error: SOROBAN_RPC_URL must be UTF-8");
            return EXIT_INVALID_ARGS;
        }
    };
    let fleet = match load_manifest(&args.config, rpc_env.as_deref()) {
        Ok(fleet) => fleet,
        Err(error) => {
            eprintln!("Error: {error}");
            return EXIT_INVALID_ARGS;
        }
    };
    let scan = ScanContractUseCase::new(SorobanRpcClient::new(fleet.rpc_url));
    let json = args.format == "json";
    let mut overall = FleetReport {
        contracts: Vec::with_capacity(fleet.contracts.len()),
        summary: Summary {
            total: fleet.contracts.len(),
            ..Summary::default()
        },
    };
    for contract in fleet.contracts {
        let id = contract.request.contract_id_strkey.clone();
        if !json {
            if let Some(name) = &contract.name {
                println!("=== {name} ({id}) ===");
            } else {
                println!("=== {id} ===");
            }
        }
        let (status, report, error) = match scan.execute(contract.request).await {
            Ok(report) => {
                crate::telemetry::record_scan_metrics(&report);
                let status = report.overall_status;
                match status {
                    RiskLevel::Ok => overall.summary.ok += 1,
                    RiskLevel::Warn => overall.summary.warn += 1,
                    RiskLevel::Critical => overall.summary.critical += 1,
                    RiskLevel::Archived => overall.summary.archived += 1,
                }
                if !json {
                    println!("{}", render_text_with_options(&report, args.no_color));
                }
                (
                    Some(status),
                    json.then(|| JsonTTLReport::from(&report)),
                    None,
                )
            }
            Err(error) => {
                overall.summary.failed += 1;
                // RPC endpoints and response bodies can contain connection credentials.
                let error = match error {
                    SentinelError::RpcConnectionTimeout(_) => {
                        "RPC connection failed or timed out".to_owned()
                    }
                    SentinelError::RpcResponseError { code, .. } => {
                        format!("RPC request failed (code {code})")
                    }
                    SentinelError::SerializationError(_) => {
                        "RPC response could not be decoded".to_owned()
                    }
                    _ => "Contract scan failed".to_owned(),
                };
                if !json {
                    println!("Error: {error}");
                }
                (None, None, Some(error))
            }
        };
        if json {
            overall.contracts.push(ContractResult {
                id,
                name: contract.name,
                status,
                report,
                error,
            });
        }
    }
    if json {
        match serde_json::to_string_pretty(&overall) {
            Ok(output) => println!("{output}"),
            Err(error) => {
                eprintln!("Error: {error}");
                return EXIT_RPC_FAILURE;
            }
        }
    } else {
        let s = &overall.summary;
        println!(
            "Overall summary: {} contracts, {} OK, {} WARN, {} CRITICAL, {} ARCHIVED, {} FAILED",
            s.total, s.ok, s.warn, s.critical, s.archived, s.failed
        );
    }
    overall.summary.exit_code()
}

async fn run_monitoring_check(id: &str, options: &crate::args::StorageScanOptions) -> i32 {
    let report = match super::top_consumers::scan_entries(id, options).await {
        Ok(report) => report,
        Err((_, error)) => {
            println!("SENTINEL UNKNOWN - {error}");
            return 3;
        }
    };
    let (code, status) = match report.overall_status {
        RiskLevel::Ok => (0, "OK"),
        RiskLevel::Warn => (1, "WARNING"),
        RiskLevel::Critical | RiskLevel::Archived => (2, "CRITICAL"),
    };
    let Some(remaining) = report
        .entries
        .iter()
        .map(|entry| entry.remaining_ledgers)
        .min()
    else {
        println!("SENTINEL UNKNOWN - no resolved storage entries");
        return 3;
    };
    let thresholds =
        super::resolve_threshold_config(options.warning_threshold, options.critical_threshold)
            .expect("scan_entries validated thresholds");
    println!(
        "SENTINEL {status} - contract {}: {} entries {} | remaining_ledgers={};{}:;{}:;0;",
        report.contract_id,
        report.entries.len(),
        if code == 0 {
            "healthy"
        } else {
            "require attention"
        },
        remaining.max(0),
        thresholds.warning_ledgers,
        thresholds.critical_ledgers
    );
    code
}
