//! Execution logic for the `sentinel audit-limits` CLI command.
//!
//! Verifies contract storage compliance against Stellar protocol ceilings:
//! - Contract instance entry size <= max_contract_data_entry_size_bytes (64 KB)
//! - WASM bytecode size <= max_contract_size_bytes (64 KB)
//! - Individual persistent storage entry sizes <= 64 KB (flags >= 80% of protocol limits)
//!
//! Outputs a compliance scorecard table with status indicators (`PASS`, `WARN`, `FAIL`)
//! and returns exit code 2 if any ceiling is breached.

use serde::{Deserialize, Serialize};
use std::time::Duration;

use sentinel_core::adapters::{parse_contract_id, parse_key_expr, SorobanRpcClient};
use sentinel_core::domain::{
    Durability, EntryType, LIMIT_WARN_THRESHOLD_PERCENT, MAX_CONTRACT_DATA_ENTRY_SIZE_BYTES,
    MAX_CONTRACT_SIZE_BYTES,
};
use sentinel_core::usecases::{ScanContractUseCase, ScanRequest};
use sentinel_core::{derive_persistent_xdr_key, SentinelError};

use crate::args::AuditLimitsArgs;
use crate::commands::scan::resolve_rpc_url;
use crate::commands::{EXIT_INVALID_ARGS, EXIT_OK, EXIT_RPC_FAILURE};

/// Exit code returned when any protocol storage ceiling is breached.
pub const EXIT_CEILING_BREACHED: i32 = 2;

/// Compliance status indicator for an audited entry or component.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum ComplianceStatus {
    Pass,
    Warn,
    Fail,
}

impl std::fmt::Display for ComplianceStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pass => write!(f, "PASS"),
            Self::Warn => write!(f, "WARN"),
            Self::Fail => write!(f, "FAIL"),
        }
    }
}

/// Scorecard row for an individual inspected contract storage item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScorecardRow {
    pub scope: String,
    pub entry_type: String,
    pub size_bytes: usize,
    pub limit_bytes: usize,
    pub percent_used: usize,
    pub status: ComplianceStatus,
    pub notes: String,
}

/// Comprehensive compliance audit scorecard report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComplianceScorecardReport {
    pub contract_id: String,
    pub network: String,
    pub latest_ledger: u32,
    pub overall_status: ComplianceStatus,
    pub rows: Vec<ScorecardRow>,
    pub total_inspected: usize,
    pub passed_count: usize,
    pub warning_count: usize,
    pub breached_count: usize,
}

/// Evaluates a single size against a ceiling limit.
pub fn evaluate_limit_compliance(
    scope: impl Into<String>,
    entry_type: impl Into<String>,
    size_bytes: usize,
    limit_bytes: usize,
) -> ScorecardRow {
    let percent_used = if limit_bytes == 0 {
        0
    } else {
        ((size_bytes as u128 * 100) / limit_bytes as u128).min(usize::MAX as u128) as usize
    };

    let (status, notes) = if size_bytes > limit_bytes {
        (
            ComplianceStatus::Fail,
            format!(
                "BREACHED ceiling by {} bytes ({}% of limit)",
                size_bytes.saturating_sub(limit_bytes),
                percent_used
            ),
        )
    } else if percent_used >= LIMIT_WARN_THRESHOLD_PERCENT {
        (
            ComplianceStatus::Warn,
            format!(
                "Approaching ceiling ({}% used, threshold {}%)",
                percent_used, LIMIT_WARN_THRESHOLD_PERCENT
            ),
        )
    } else {
        (
            ComplianceStatus::Pass,
            format!("Within protocol limit ({}% used)", percent_used),
        )
    };

    ScorecardRow {
        scope: scope.into(),
        entry_type: entry_type.into(),
        size_bytes,
        limit_bytes,
        percent_used,
        status,
        notes,
    }
}

fn render_scorecard_table(report: &ComplianceScorecardReport) -> String {
    fn truncate_cell(value: &str, max_chars: usize) -> String {
        let chars: Vec<char> = value.chars().collect();
        if chars.len() <= max_chars {
            value.to_string()
        } else {
            chars
                .iter()
                .take(max_chars.saturating_sub(3))
                .copied()
                .collect::<String>()
                + "..."
        }
    }

    let mut out = format!(
        "\nStorage Protocol Ceilings Scorecard: {}\nNetwork: {} | Ledger: {} | Overall: {}\n",
        report.contract_id, report.network, report.latest_ledger, report.overall_status
    );
    out.push_str("+-------------------------------+------------------+------------+------------+--------+--------+------------------------------------------------+\n");
    out.push_str("| Target / Identifier           | Type             | Size Bytes | Limit Byte | % Used | Status | Notes                                          |\n");
    out.push_str("+-------------------------------+------------------+------------+------------+--------+--------+------------------------------------------------+\n");
    for row in &report.rows {
        out.push_str(&format!(
            "| {:<29} | {:<16} | {:>10} | {:>10} | {:>5}% | {:<6} | {:<46} |\n",
            truncate_cell(&row.scope, 29),
            row.entry_type,
            row.size_bytes,
            row.limit_bytes,
            row.percent_used,
            row.status,
            truncate_cell(&row.notes, 46)
        ));
    }
    out.push_str("+-------------------------------+------------------+------------+------------+--------+--------+------------------------------------------------+\n");
    out.push_str(&format!(
        "Summary: {} items audited | {} PASS | {} WARN | {} FAIL (ceilings breached)\n",
        report.total_inspected, report.passed_count, report.warning_count, report.breached_count
    ));
    out
}

pub async fn run_audit_limits(args: AuditLimitsArgs) -> i32 {
    if let Some(timeout) = args.rpc_timeout {
        if !(1..=120).contains(&timeout) {
            eprintln!("Error: --rpc-timeout must be between 1 and 120 seconds");
            return EXIT_INVALID_ARGS;
        }
    }
    if let Some(retries) = args.rpc_max_retries {
        if retries > 10 {
            eprintln!("Error: --rpc-max-retries must be between 0 and 10");
            return EXIT_INVALID_ARGS;
        }
    }

    let rpc_url = match resolve_rpc_url(&args.network, args.rpc_url.as_deref()) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Error: {}", e);
            return EXIT_INVALID_ARGS;
        }
    };
    if let Err(e) = sentinel_core::adapters::validate_rpc_url(&rpc_url) {
        eprintln!("Error: {}", e);
        return EXIT_INVALID_ARGS;
    }

    let mut rpc = SorobanRpcClient::new_with_timeout(
        rpc_url,
        Duration::from_secs(args.rpc_timeout.unwrap_or(10)),
    );
    if let Some(retries) = args.rpc_max_retries {
        let mut policy = rpc.retry_policy().clone();
        policy.max_retries = retries;
        rpc = rpc.with_retry_policy(policy);
    }

    let contract_id = match parse_contract_id(&args.contract_id) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Error: {}", e);
            return EXIT_INVALID_ARGS;
        }
    };

    // Validate and prepare persistent keys
    for xdr in &args.key_xdr {
        if let Err(e) = derive_persistent_xdr_key(&contract_id, xdr) {
            eprintln!("Error: invalid XDR key '{}': {}", xdr, e);
            return EXIT_INVALID_ARGS;
        }
    }
    for expr in &args.key_expr {
        if let Err(e) = parse_key_expr(expr)
            .and_then(|v| sentinel_core::derive_persistent_key_from_scval(&contract_id, v))
        {
            eprintln!("Error: invalid key expression '{}': {}", expr, e);
            return EXIT_INVALID_ARGS;
        }
    }

    let request = ScanRequest::new(&args.contract_id)
        .with_network(&args.network)
        .with_persistent_symbols(args.key.clone())
        .with_persistent_xdr_keys(args.key_xdr.clone())
        .with_persistent_key_exprs(args.key_expr.clone());

    let usecase = ScanContractUseCase::new(rpc);
    let scan = match usecase.execute(request).await {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Error: {}", e);
            return match e {
                SentinelError::InvalidContractAddress(_)
                | SentinelError::InvalidXdrKey(_)
                | SentinelError::InvalidRpcUrl(_) => EXIT_INVALID_ARGS,
                _ => EXIT_RPC_FAILURE,
            };
        }
    };

    let mut rows = Vec::new();

    // 1. Check contract instance entry size
    let instance_entry = scan.entries.iter().find(|e| {
        e.entry_type == EntryType::ContractInstance || e.durability == Durability::Instance
    });
    if let Some(inst) = instance_entry {
        rows.push(evaluate_limit_compliance(
            "ContractInstance",
            "instance",
            inst.byte_size,
            MAX_CONTRACT_DATA_ENTRY_SIZE_BYTES,
        ));
    } else {
        rows.push(ScorecardRow {
            scope: "ContractInstance".to_string(),
            entry_type: "instance".to_string(),
            size_bytes: 0,
            limit_bytes: MAX_CONTRACT_DATA_ENTRY_SIZE_BYTES,
            percent_used: 0,
            status: ComplianceStatus::Warn,
            notes: "Instance entry not found".to_string(),
        });
    }

    // 2. Check WASM bytecode size against network max_contract_size_bytes
    let code_entry = scan
        .entries
        .iter()
        .find(|e| e.entry_type == EntryType::ContractCode || e.durability == Durability::Code);
    if let Some(wasm) = code_entry {
        if let Some(code_size_bytes) = wasm.contract_code_size_bytes {
            rows.push(evaluate_limit_compliance(
                "ContractCode (WASM)",
                "code",
                code_size_bytes,
                MAX_CONTRACT_SIZE_BYTES,
            ));
        } else {
            rows.push(ScorecardRow {
                scope: "ContractCode (WASM)".to_string(),
                entry_type: "code".to_string(),
                size_bytes: 0,
                limit_bytes: MAX_CONTRACT_SIZE_BYTES,
                percent_used: 0,
                status: ComplianceStatus::Warn,
                notes: "WASM bytecode was not returned; size not assessed".to_string(),
            });
        }
    } else {
        rows.push(ScorecardRow {
            scope: "ContractCode (WASM)".to_string(),
            entry_type: "code".to_string(),
            size_bytes: 0,
            limit_bytes: MAX_CONTRACT_SIZE_BYTES,
            percent_used: 0,
            status: if scan.sac_asset_info.is_some() {
                ComplianceStatus::Pass
            } else {
                ComplianceStatus::Warn
            },
            notes: if scan.sac_asset_info.is_some() {
                "Not applicable to Stellar Asset Contracts".to_string()
            } else {
                "Bytecode entry not found; size not assessed".to_string()
            },
        });
    }

    // 3. Check individual persistent storage entry sizes
    let persistent_entries: Vec<_> = scan
        .entries
        .iter()
        .filter(|entry| {
            entry.durability == Durability::Persistent
                || entry.entry_type == EntryType::PersistentData
        })
        .collect();
    if persistent_entries.is_empty() {
        rows.push(ScorecardRow {
            scope: "Persistent storage".to_string(),
            entry_type: "persistent".to_string(),
            size_bytes: 0,
            limit_bytes: MAX_CONTRACT_DATA_ENTRY_SIZE_BYTES,
            percent_used: 0,
            status: ComplianceStatus::Warn,
            notes: "No persistent keys selected; pass --key, --key-xdr, or --key-expr".to_string(),
        });
    }

    for entry in persistent_entries {
        if entry.live_until_ledger == 0 {
            rows.push(ScorecardRow {
                scope: entry.key_identifier.clone(),
                entry_type: "persistent".to_string(),
                size_bytes: 0,
                limit_bytes: MAX_CONTRACT_DATA_ENTRY_SIZE_BYTES,
                percent_used: 0,
                status: ComplianceStatus::Warn,
                notes: "Persistent entry not found; size not assessed".to_string(),
            });
        } else {
            rows.push(evaluate_limit_compliance(
                entry.key_identifier.clone(),
                "persistent",
                entry.byte_size,
                MAX_CONTRACT_DATA_ENTRY_SIZE_BYTES,
            ));
        }
    }

    let passed_count = rows
        .iter()
        .filter(|r| r.status == ComplianceStatus::Pass)
        .count();
    let warning_count = rows
        .iter()
        .filter(|r| r.status == ComplianceStatus::Warn)
        .count();
    let breached_count = rows
        .iter()
        .filter(|r| r.status == ComplianceStatus::Fail)
        .count();

    let overall_status = if breached_count > 0 {
        ComplianceStatus::Fail
    } else if warning_count > 0 {
        ComplianceStatus::Warn
    } else {
        ComplianceStatus::Pass
    };

    let report = ComplianceScorecardReport {
        contract_id: args.contract_id,
        network: args.network,
        latest_ledger: scan.latest_ledger,
        overall_status,
        total_inspected: rows.len(),
        passed_count,
        warning_count,
        breached_count,
        rows,
    };

    if args.format.eq_ignore_ascii_case("json") {
        match serde_json::to_string_pretty(&report) {
            Ok(json) => println!("{}", json),
            Err(e) => {
                eprintln!("Error serializing JSON scorecard: {}", e);
                return EXIT_INVALID_ARGS;
            }
        }
    } else {
        println!("{}", render_scorecard_table(&report));
    }

    if overall_status == ComplianceStatus::Fail {
        EXIT_CEILING_BREACHED
    } else {
        EXIT_OK
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_evaluate_limit_compliance_thresholds() {
        // Safe size (< 80% of 65536 = 52428) -> PASS
        let row_pass = evaluate_limit_compliance("entry1", "persistent", 10_000, 65_536);
        assert_eq!(row_pass.status, ComplianceStatus::Pass);

        // The first whole-byte size at or above 80% of 65536 -> WARN
        let row_at_threshold = evaluate_limit_compliance("entry2a", "persistent", 52_429, 65_536);
        assert_eq!(row_at_threshold.status, ComplianceStatus::Warn);

        // Warning size (>= 80%, <= 65536) -> WARN
        let row_warn = evaluate_limit_compliance("entry2", "persistent", 53_000, 65_536);
        assert_eq!(row_warn.status, ComplianceStatus::Warn);

        // Limit breach (> 65536) -> FAIL
        let row_fail = evaluate_limit_compliance("entry3", "persistent", 65_537, 65_536);
        assert_eq!(row_fail.status, ComplianceStatus::Fail);

        let row_huge =
            evaluate_limit_compliance("entry4", "persistent", usize::MAX, usize::MAX - 1);
        assert_eq!(row_huge.status, ComplianceStatus::Fail);
    }
}
