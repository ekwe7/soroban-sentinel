use std::collections::BTreeMap;
use std::fs;

use sentinel_core::domain::RiskLevel;
use serde::Serialize;

use crate::args::DiffReportsArgs;
use crate::commands::{EXIT_CRITICAL_OR_ARCHIVED, EXIT_INVALID_ARGS, EXIT_OK};
use crate::formatters::JsonTTLReport;

#[derive(Debug, Serialize)]
struct ReportDiff {
    old_contract_id: String,
    new_contract_id: String,
    old_latest_ledger: u32,
    new_latest_ledger: u32,
    entries: Vec<EntryChange>,
    added: usize,
    removed: usize,
    regressions: usize,
}

#[derive(Debug, Serialize)]
struct EntryChange {
    key_identifier: String,
    change: String,
    remaining_ledgers_delta: Option<i64>,
    live_until_ledger_delta: Option<i64>,
    old_status: Option<String>,
    new_status: Option<String>,
}

pub fn run_diff_reports(args: DiffReportsArgs) -> i32 {
    let old = match load(&args.old) {
        Ok(report) => report,
        Err(error) => return invalid(error),
    };
    let new = match load(&args.new) {
        Ok(report) => report,
        Err(error) => return invalid(error),
    };
    if old.contract_id != new.contract_id {
        return invalid(format!(
            "report contract IDs do not match: '{}' vs '{}'",
            old.contract_id, new.contract_id
        ));
    }
    if !old.network.eq_ignore_ascii_case(&new.network) {
        return invalid(format!(
            "report networks do not match: '{}' vs '{}'",
            old.network, new.network
        ));
    }
    let old_entries = old
        .entries
        .iter()
        .map(|entry| (entry.identifier.clone(), entry))
        .collect::<BTreeMap<_, _>>();
    let new_entries = new
        .entries
        .iter()
        .map(|entry| (entry.identifier.clone(), entry))
        .collect::<BTreeMap<_, _>>();
    let mut entries = Vec::new();
    let mut added = 0;
    let mut removed = 0;
    let mut regressions = 0;
    for (id, current) in &new_entries {
        if let Some(previous) = old_entries.get(id) {
            let old_status = parse_status(&previous.status);
            let new_status = parse_status(&current.status);
            let change = if status_severity(new_status) > status_severity(old_status) {
                regressions += 1;
                "regressed"
            } else if status_severity(new_status) < status_severity(old_status) {
                "improved"
            } else {
                "changed"
            };
            entries.push(EntryChange {
                key_identifier: id.clone(),
                change: change.into(),
                remaining_ledgers_delta: Some(
                    current.remaining_ledgers - previous.remaining_ledgers,
                ),
                live_until_ledger_delta: Some(
                    i64::from(current.live_until_ledger) - i64::from(previous.live_until_ledger),
                ),
                old_status: Some(previous.status.clone()),
                new_status: Some(current.status.clone()),
            });
        } else {
            added += 1;
            entries.push(EntryChange {
                key_identifier: id.clone(),
                change: "added".into(),
                remaining_ledgers_delta: None,
                live_until_ledger_delta: None,
                old_status: None,
                new_status: Some(current.status.clone()),
            });
        }
    }
    for (id, previous) in &old_entries {
        if !new_entries.contains_key(id) {
            removed += 1;
            regressions += 1;
            entries.push(EntryChange {
                key_identifier: id.clone(),
                change: "removed".into(),
                remaining_ledgers_delta: Some(-previous.remaining_ledgers),
                live_until_ledger_delta: Some(-i64::from(previous.live_until_ledger)),
                old_status: Some(previous.status.clone()),
                new_status: None,
            });
        }
    }
    let diff = ReportDiff {
        old_contract_id: old.contract_id,
        new_contract_id: new.contract_id,
        old_latest_ledger: old.latest_ledger,
        new_latest_ledger: new.latest_ledger,
        entries,
        added,
        removed,
        regressions,
    };
    if args.format == "json" {
        println!(
            "{}",
            serde_json::to_string_pretty(&diff).expect("diff is serializable")
        );
    } else {
        println!(
            "{} -> {} ({})",
            diff.old_contract_id, diff.new_contract_id, diff.new_latest_ledger
        );
        for entry in &diff.entries {
            println!("{}: {}", entry.key_identifier, entry.change);
        }
        println!(
            "Summary: {} added, {} removed, {} regressions",
            diff.added, diff.removed, diff.regressions
        );
    }
    if args.fail_on_regression && diff.regressions > 0 {
        EXIT_CRITICAL_OR_ARCHIVED
    } else {
        EXIT_OK
    }
}

fn load(path: &std::path::Path) -> Result<JsonTTLReport, String> {
    let contents =
        fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?;
    serde_json::from_str(&contents)
        .map_err(|error| format!("invalid TTL report '{}': {error}", path.display()))
}

fn parse_status(status: &str) -> RiskLevel {
    match status.to_ascii_uppercase().as_str() {
        "WARN" => RiskLevel::Warn,
        "CRITICAL" => RiskLevel::Critical,
        "ARCHIVED" => RiskLevel::Archived,
        _ => RiskLevel::Ok,
    }
}
fn status_severity(status: RiskLevel) -> u8 {
    match status {
        RiskLevel::Ok => 0,
        RiskLevel::Warn => 1,
        RiskLevel::Critical => 2,
        RiskLevel::Archived => 3,
    }
}
fn invalid(error: impl std::fmt::Display) -> i32 {
    eprintln!("Error: {error}");
    EXIT_INVALID_ARGS
}
