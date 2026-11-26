use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::{SystemTime, UNIX_EPOCH};

use sentinel_core::adapters::{parse_contract_id, safe_xdr_limits, SorobanRpcClient};
use sentinel_core::domain::{
    evaluate_risk_for_durability, project_exhaustion, Durability, RiskLevel, ThresholdConfig,
};
use sentinel_core::usecases::{ScanContractUseCase, ScanRequest, TTLReport};
use sentinel_core::{
    derive_contract_instance_key, derive_persistent_key_from_scval,
    derive_temporary_key_from_scval, parse_key_expr, LedgerReader, RawLedgerEntry, SentinelError,
};
use serde::Serialize;
use stellar_xdr::curr::{
    Hash, LedgerEntry, LedgerEntryData, Limits, ReadXdr, ScAddress, ScSymbol, ScVal, WriteXdr,
};

use super::{
    resolve_rpc_url, resolve_threshold_config, trend::format_date_utc, EXIT_INVALID_ARGS, EXIT_OK,
    EXIT_RPC_FAILURE,
};
use crate::args::PrunePlanArgs;

#[derive(Serialize)]
struct PlannedEntry {
    identifier: String,
    durability: String,
    expiration_ledger: u32,
    remaining_ledgers: i64,
    estimated_remaining_days: f64,
    estimated_expiration_timestamp: Option<i64>,
    estimated_expiration_date: Option<String>,
    status: String,
    safety: String,
    referenced_by: Vec<String>,
}

#[derive(Serialize)]
struct CalendarMilestone {
    milestone: usize,
    expiration_ledger: u32,
    estimated_expiration_date: Option<String>,
    entries: Vec<String>,
    cumulative_entries: usize,
}

#[derive(Serialize)]
struct PrunePlanReport<'a> {
    contract_id: &'a str,
    network: &'a str,
    latest_ledger: u32,
    ledger_velocity_seconds: f64,
    anchor_source: &'a str,
    anchor_timestamp: i64,
    total_marked_entries: usize,
    warning_count: usize,
    entries: Vec<PlannedEntry>,
    calendar: Vec<CalendarMilestone>,
    warnings: Vec<String>,
}

#[derive(Default)]
struct DerivedKeys {
    by_identifier: BTreeMap<String, String>,
    scval_by_identifier: BTreeMap<String, ScVal>,
}

type CommandError = (i32, &'static str);

fn dedup(values: Vec<String>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    values
        .into_iter()
        .filter(|value| seen.insert(value.clone()))
        .collect()
}

fn now_unix_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}

fn truncate(value: &str, width: usize) -> String {
    let mut output: String = value.chars().take(width).collect();
    if value.chars().count() > width {
        output.truncate(width.saturating_sub(1));
        output.push('~');
    }
    output
}

fn symbol_scval(symbol: &str) -> Result<ScVal, CommandError> {
    let sc_symbol = ScSymbol(
        symbol
            .try_into()
            .map_err(|_| (EXIT_INVALID_ARGS, "invalid storage key symbol"))?,
    );
    Ok(ScVal::Symbol(sc_symbol))
}

fn insert_persistent(
    derived: &mut DerivedKeys,
    contract: &[u8; 32],
    identifier: String,
    key: ScVal,
) -> Result<(), CommandError> {
    let key_xdr = derive_persistent_key_from_scval(contract, key.clone())
        .map_err(|_| (EXIT_INVALID_ARGS, "invalid persistent storage key"))?;
    derived.by_identifier.insert(identifier.clone(), key_xdr);
    derived.scval_by_identifier.insert(identifier, key);
    Ok(())
}

fn derive_keys(contract: &[u8; 32], args: &PrunePlanArgs) -> Result<DerivedKeys, CommandError> {
    let mut derived = DerivedKeys::default();
    let instance_key = derive_contract_instance_key(contract)
        .map_err(|_| (EXIT_INVALID_ARGS, "invalid contract instance key"))?;
    derived
        .by_identifier
        .insert("instance".to_string(), instance_key);
    let contract_address = ScVal::Address(ScAddress::Contract(Hash(*contract)));
    derived
        .scval_by_identifier
        .insert("instance".to_string(), contract_address.clone());
    derived
        .scval_by_identifier
        .insert("code_wasm".to_string(), contract_address);

    for symbol in &args.key {
        insert_persistent(
            &mut derived,
            contract,
            format!("persistent:{symbol}"),
            symbol_scval(symbol)?,
        )?;
    }
    for xdr in &args.key_xdr {
        let key = ScVal::from_xdr_base64(xdr, safe_xdr_limits())
            .map_err(|_| (EXIT_INVALID_ARGS, "invalid base64 ScVal storage key"))?;
        insert_persistent(&mut derived, contract, format!("persistent_xdr:{xdr}"), key)?;
    }
    for expression in &args.key_expr {
        let key = parse_key_expr(expression)
            .map_err(|_| (EXIT_INVALID_ARGS, "invalid storage key expression"))?;
        insert_persistent(
            &mut derived,
            contract,
            format!("persistent:{expression}"),
            key,
        )?;
    }
    for symbol in &args.expire_key {
        insert_persistent(
            &mut derived,
            contract,
            format!("persistent:{symbol}"),
            symbol_scval(symbol)?,
        )?;
    }
    for xdr in &args.expire_key_xdr {
        let key = ScVal::from_xdr_base64(xdr, safe_xdr_limits())
            .map_err(|_| (EXIT_INVALID_ARGS, "invalid base64 ScVal storage key"))?;
        insert_persistent(&mut derived, contract, format!("persistent_xdr:{xdr}"), key)?;
    }
    for expression in &args.expire_key_expr {
        let key = parse_key_expr(expression)
            .map_err(|_| (EXIT_INVALID_ARGS, "invalid storage key expression"))?;
        insert_persistent(
            &mut derived,
            contract,
            format!("persistent:{expression}"),
            key,
        )?;
    }
    for symbol in &args.temp_key {
        let key = symbol_scval(symbol)?;
        let key_xdr = derive_temporary_key_from_scval(contract, key.clone())
            .map_err(|_| (EXIT_INVALID_ARGS, "invalid temporary storage key"))?;
        let identifier = format!("temporary:{symbol}");
        derived.by_identifier.insert(identifier.clone(), key_xdr);
        derived.scval_by_identifier.insert(identifier, key);
    }
    for expression in &args.temp_key_expr {
        let key = parse_key_expr(expression)
            .map_err(|_| (EXIT_INVALID_ARGS, "invalid temporary key expression"))?;
        let key_xdr = derive_temporary_key_from_scval(contract, key.clone())
            .map_err(|_| (EXIT_INVALID_ARGS, "invalid temporary storage key"))?;
        let identifier = format!("temporary:{expression}");
        derived.by_identifier.insert(identifier.clone(), key_xdr);
        derived.scval_by_identifier.insert(identifier, key);
    }
    Ok(derived)
}

fn marked_identifiers(args: &PrunePlanArgs, report: &TTLReport) -> BTreeSet<String> {
    let explicit = !args.expire_key.is_empty()
        || !args.expire_key_xdr.is_empty()
        || !args.expire_key_expr.is_empty()
        || !args.temp_key.is_empty()
        || !args.temp_key_expr.is_empty();
    if explicit {
        let mut marked = BTreeSet::new();
        for symbol in &args.expire_key {
            marked.insert(format!("persistent:{symbol}"));
        }
        for xdr in &args.expire_key_xdr {
            marked.insert(format!("persistent_xdr:{xdr}"));
        }
        for expression in &args.expire_key_expr {
            marked.insert(format!("persistent:{expression}"));
        }
        for symbol in &args.temp_key {
            marked.insert(format!("temporary:{symbol}"));
        }
        for expression in &args.temp_key_expr {
            marked.insert(format!("temporary:{expression}"));
        }
        return marked;
    }

    let persistent: BTreeSet<String> = report
        .entries
        .iter()
        .filter(|entry| entry.durability == Durability::Persistent)
        .map(|entry| entry.key_identifier.clone())
        .collect();
    if persistent.is_empty() {
        report
            .entries
            .iter()
            .map(|entry| entry.key_identifier.clone())
            .collect()
    } else {
        persistent
    }
}

async fn fetch_raw_entries<R: LedgerReader>(
    usecase: &ScanContractUseCase<R>,
    derived: &DerivedKeys,
    identifiers: &BTreeSet<String>,
) -> Result<HashMap<String, RawLedgerEntry>, CommandError> {
    let keys: Vec<String> = identifiers
        .iter()
        .filter_map(|identifier| derived.by_identifier.get(identifier).cloned())
        .collect();
    if keys.is_empty() {
        return Ok(HashMap::new());
    }
    let entries = usecase
        .reader()
        .get_ledger_entries(&keys)
        .await
        .map_err(|_| {
            (
                EXIT_RPC_FAILURE,
                "failed to read storage entries for reference analysis",
            )
        })?;
    Ok(entries
        .into_iter()
        .map(|entry| (entry.key_xdr_base64.clone(), entry))
        .collect())
}

fn value_bytes(entry_xdr_base64: &str) -> Option<Vec<u8>> {
    let data = match LedgerEntry::from_xdr_base64(entry_xdr_base64, safe_xdr_limits()) {
        Ok(entry) => entry.data,
        Err(_) => LedgerEntryData::from_xdr_base64(entry_xdr_base64, safe_xdr_limits()).ok()?,
    };
    match data {
        LedgerEntryData::ContractData(contract_data) => {
            contract_data.val.to_xdr(Limits::none()).ok()
        }
        _ => None,
    }
}

fn active_value_bytes(
    report: &TTLReport,
    marked: &BTreeSet<String>,
    derived: &DerivedKeys,
    raw: &HashMap<String, RawLedgerEntry>,
) -> HashMap<String, Vec<u8>> {
    let mut active = HashMap::new();
    for entry in &report.entries {
        if marked.contains(&entry.key_identifier) || entry.durability != Durability::Persistent {
            continue;
        }
        let Some(key_xdr) = derived.by_identifier.get(&entry.key_identifier) else {
            continue;
        };
        let Some(raw_entry) = raw.get(key_xdr) else {
            continue;
        };
        if let Some(bytes) = value_bytes(&raw_entry.entry_xdr_base64) {
            active.insert(entry.key_identifier.clone(), bytes);
        }
    }
    active
}

fn references_for(
    identifier: &str,
    derived: &DerivedKeys,
    active: &HashMap<String, Vec<u8>>,
) -> Vec<String> {
    let Some(pattern) = derived
        .scval_by_identifier
        .get(identifier)
        .and_then(|value| value.to_xdr(Limits::none()).ok())
    else {
        return Vec::new();
    };
    let mut references: Vec<String> = active
        .iter()
        .filter(|(_, bytes)| contains_bytes(bytes, &pattern))
        .map(|(referrer, _)| referrer.clone())
        .collect();
    references.sort();
    references
}

struct PlannedInput {
    identifier: String,
    durability: Durability,
    live_until_ledger: u32,
    remaining_ledgers: i64,
    status: RiskLevel,
    referenced_by: Vec<String>,
}

fn make_planned_entry(
    input: PlannedInput,
    report: &TTLReport,
    anchor_timestamp: i64,
) -> PlannedEntry {
    let timestamp = project_exhaustion(
        &report.burn_rate,
        input.remaining_ledgers,
        Some(anchor_timestamp),
    )
    .and_then(|projection| projection.exhaustion_timestamp);
    let estimated_expiration_date = timestamp.map(format_date_utc);
    let estimated_remaining_days = if input.remaining_ledgers > 0 {
        (input.remaining_ledgers as f64 * report.ledger_velocity_seconds / 86_400.0 * 100.0).round()
            / 100.0
    } else {
        0.0
    };
    let safety = if !input.referenced_by.is_empty() {
        "warning"
    } else if input.remaining_ledgers <= 0 {
        "expired"
    } else {
        "ok"
    };
    PlannedEntry {
        identifier: input.identifier,
        durability: input.durability.to_string().to_lowercase(),
        expiration_ledger: input.live_until_ledger,
        remaining_ledgers: input.remaining_ledgers,
        estimated_remaining_days,
        estimated_expiration_timestamp: timestamp,
        estimated_expiration_date,
        status: input.status.to_string().to_lowercase(),
        safety: safety.to_string(),
        referenced_by: input.referenced_by,
    }
}

fn planned_entries(
    report: &TTLReport,
    marked: &BTreeSet<String>,
    derived: &DerivedKeys,
    raw: &HashMap<String, RawLedgerEntry>,
    anchor_timestamp: i64,
    thresholds: &ThresholdConfig,
) -> Vec<PlannedEntry> {
    let active = active_value_bytes(report, marked, derived, raw);
    let mut entries = Vec::new();
    for entry in &report.entries {
        if !marked.contains(&entry.key_identifier) {
            continue;
        }
        let referenced_by = references_for(&entry.key_identifier, derived, &active);
        entries.push(make_planned_entry(
            PlannedInput {
                identifier: entry.key_identifier.clone(),
                durability: entry.durability,
                live_until_ledger: entry.live_until_ledger,
                remaining_ledgers: entry.remaining_ledgers,
                status: entry.status,
                referenced_by,
            },
            report,
            anchor_timestamp,
        ));
    }
    for identifier in marked {
        if !identifier.starts_with("temporary:")
            || report
                .entries
                .iter()
                .any(|entry| entry.key_identifier == *identifier)
        {
            continue;
        }
        let Some(key_xdr) = derived.by_identifier.get(identifier) else {
            continue;
        };
        let Some(raw_entry) = raw.get(key_xdr) else {
            continue;
        };
        let live_until = raw_entry.live_until_ledger_seq;
        let remaining = live_until as i64 - report.latest_ledger as i64;
        let status = evaluate_risk_for_durability(remaining, Durability::Temporary, thresholds);
        let referenced_by = references_for(identifier, derived, &active);
        entries.push(make_planned_entry(
            PlannedInput {
                identifier: identifier.clone(),
                durability: Durability::Temporary,
                live_until_ledger: live_until,
                remaining_ledgers: remaining,
                status,
                referenced_by,
            },
            report,
            anchor_timestamp,
        ));
    }
    entries.sort_by(|left, right| {
        left.expiration_ledger
            .cmp(&right.expiration_ledger)
            .then_with(|| left.identifier.cmp(&right.identifier))
    });
    entries
}

fn build_calendar(entries: &[PlannedEntry]) -> Vec<CalendarMilestone> {
    let mut milestones: Vec<CalendarMilestone> = Vec::new();
    let mut cumulative = 0;
    for entry in entries {
        cumulative += 1;
        if let Some(last) = milestones.last_mut() {
            if last.expiration_ledger == entry.expiration_ledger {
                last.entries.push(entry.identifier.clone());
                last.cumulative_entries = cumulative;
                continue;
            }
        }
        milestones.push(CalendarMilestone {
            milestone: milestones.len() + 1,
            expiration_ledger: entry.expiration_ledger,
            estimated_expiration_date: entry.estimated_expiration_date.clone(),
            entries: vec![entry.identifier.clone()],
            cumulative_entries: cumulative,
        });
    }
    milestones
}

fn render_text(report: &PrunePlanReport<'_>) -> String {
    let mut output = String::new();
    output.push_str(&format!(
        "Prune Plan: {} | Network: {} | Latest Ledger: {}\n",
        report.contract_id, report.network, report.latest_ledger
    ));
    output.push_str(&format!(
        "Ledger velocity: {:.2}s | Calendar anchor: {} ({})\n",
        report.ledger_velocity_seconds,
        format_date_utc(report.anchor_timestamp),
        report.anchor_source
    ));
    output.push_str(&format!(
        "Marked entries: {} | Warnings: {}\n",
        report.total_marked_entries, report.warning_count
    ));
    if !report.warnings.is_empty() {
        output.push_str("Warnings:\n");
        for warning in &report.warnings {
            output.push_str(&format!("  WARNING: {warning}\n"));
        }
    }
    output.push('\n');
    output.push_str(&format!(
        "{:<34} {:<10} {:>16} {:>16} {:<12} {:<9}\n",
        "Entry", "Durability", "Expires Ledger", "Remaining", "Est. Date", "Safety"
    ));
    output.push_str(&"-".repeat(104));
    output.push('\n');
    for entry in &report.entries {
        output.push_str(&format!(
            "{:<34} {:<10} {:>16} {:>16} {:<12} {:<9}\n",
            truncate(&entry.identifier, 34),
            entry.durability,
            entry.expiration_ledger,
            entry.remaining_ledgers,
            entry
                .estimated_expiration_date
                .as_deref()
                .unwrap_or("expired"),
            entry.safety
        ));
    }
    output.push_str("\nExpiration Calendar (sequential drop-off milestones)\n");
    output.push_str(&format!(
        "{:>3} {:>16} {:<12} {:<34} {:>10}\n",
        "#", "Ledger", "Date", "Entries", "Cumulative"
    ));
    output.push_str(&"-".repeat(80));
    output.push('\n');
    for milestone in &report.calendar {
        output.push_str(&format!(
            "{:>3} {:>16} {:<12} {:<34} {:>10}\n",
            milestone.milestone,
            milestone.expiration_ledger,
            milestone
                .estimated_expiration_date
                .as_deref()
                .unwrap_or("expired"),
            truncate(&milestone.entries.join(", "), 34),
            milestone.cumulative_entries
        ));
    }
    output
}

pub async fn run_prune_plan(args: PrunePlanArgs) -> i32 {
    let contract_bytes = match parse_contract_id(&args.contract_id) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("Error: {error}");
            return EXIT_INVALID_ARGS;
        }
    };

    let rpc_url = match resolve_rpc_url(&args.network, args.rpc_url.as_deref()) {
        Ok(url) => url,
        Err(error) => {
            eprintln!("Error: {error}");
            return EXIT_INVALID_ARGS;
        }
    };
    let thresholds = match resolve_threshold_config(args.warning_threshold, args.critical_threshold)
    {
        Ok(config) => config,
        Err(error) => {
            eprintln!("Error: {error}");
            return EXIT_INVALID_ARGS;
        }
    };

    let symbols = dedup(
        args.key
            .iter()
            .chain(args.expire_key.iter())
            .cloned()
            .collect(),
    );
    let xdr_keys = dedup(
        args.key_xdr
            .iter()
            .chain(args.expire_key_xdr.iter())
            .cloned()
            .collect(),
    );
    let expressions = dedup(
        args.key_expr
            .iter()
            .chain(args.expire_key_expr.iter())
            .cloned()
            .collect(),
    );

    let derived = match derive_keys(&contract_bytes, &args) {
        Ok(keys) => keys,
        Err((code, message)) => {
            eprintln!("Error: {message}");
            return code;
        }
    };

    let request = ScanRequest::new(&args.contract_id)
        .with_network(&args.network)
        .with_threshold_config(thresholds)
        .with_persistent_symbols(symbols)
        .with_persistent_xdr_keys(xdr_keys)
        .with_persistent_key_exprs(expressions);
    let usecase = ScanContractUseCase::new(SorobanRpcClient::new(rpc_url));
    let report = match usecase.execute(request).await {
        Ok(report) => report,
        Err(error) => {
            eprintln!("Error: {error}");
            return match error {
                SentinelError::InvalidContractAddress(_)
                | SentinelError::InvalidXdrKey(_)
                | SentinelError::InvalidRpcUrl(_)
                | SentinelError::InvalidThresholdConfig(_) => EXIT_INVALID_ARGS,
                _ => EXIT_RPC_FAILURE,
            };
        }
    };

    let marked = marked_identifiers(&args, &report);
    let identifiers: BTreeSet<String> = marked
        .iter()
        .cloned()
        .chain(
            report
                .entries
                .iter()
                .map(|entry| entry.key_identifier.clone()),
        )
        .collect();
    let raw = match fetch_raw_entries(&usecase, &derived, &identifiers).await {
        Ok(raw) => raw,
        Err((code, message)) => {
            eprintln!("Error: {message}");
            return code;
        }
    };

    let (anchor_timestamp, anchor_source) = match report.burn_rate.latest_close_time {
        Some(timestamp) => (timestamp, "ledger_close"),
        None => (now_unix_seconds(), "wall_clock"),
    };
    let entries = planned_entries(
        &report,
        &marked,
        &derived,
        &raw,
        anchor_timestamp,
        &thresholds,
    );
    let calendar = build_calendar(&entries);
    let warnings: Vec<String> = entries
        .iter()
        .filter(|entry| !entry.referenced_by.is_empty())
        .map(|entry| {
            format!(
                "'{}' is marked for expiration but is referenced by active persistent key(s): {}",
                entry.identifier,
                entry.referenced_by.join(", ")
            )
        })
        .collect();
    let output = PrunePlanReport {
        contract_id: &report.contract_id,
        network: &report.network,
        latest_ledger: report.latest_ledger,
        ledger_velocity_seconds: report.ledger_velocity_seconds,
        anchor_source,
        anchor_timestamp,
        total_marked_entries: entries.len(),
        warning_count: warnings.len(),
        entries,
        calendar,
        warnings,
    };

    if args.format == "json" {
        match serde_json::to_string_pretty(&output) {
            Ok(json) => println!("{json}"),
            Err(_) => {
                eprintln!("Error: report serialization failed");
                return EXIT_RPC_FAILURE;
            }
        }
    } else {
        print!("{}", render_text(&output));
    }
    EXIT_OK
}
