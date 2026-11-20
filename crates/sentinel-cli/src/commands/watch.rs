//! Execution logic for `sentinel watch`: scan one contract on an interval and print only what
//! changed between scans.

use std::collections::HashSet;
use std::future::Future;
use std::io::Write;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sentinel_core::domain::{Durability, RiskLevel};
use sentinel_core::ports::LedgerReader;
use sentinel_core::usecases::{ScanContractUseCase, ScanRequest, StorageEntryReport, TTLReport};
use serde_json::json;
use tokio::time::MissedTickBehavior;

use crate::args::{WatchArgs, WatchFormat};
use crate::commands::scan::{
    entry_size_limit_note, exit_code_for_error, exit_code_for_status, is_rpc_failure, prepare_scan,
};
use crate::commands::{EXIT_INVALID_ARGS, EXIT_OK, EXIT_RPC_FAILURE};
use crate::formatters::text::format_entry_description;
use crate::formatters::{format_with_commas, render_text, JsonTTLReport};

/// Shortest allowed `--interval`, in seconds.
pub const MIN_WATCH_INTERVAL_SECS: u64 = 10;
/// Longest allowed `--interval`, in seconds (24 hours).
pub const MAX_WATCH_INTERVAL_SECS: u64 = 24 * 60 * 60;
/// Largest allowed `--max-consecutive-failures`.
pub const MAX_CONSECUTIVE_FAILURES_LIMIT: u32 = 100;

/// What changed for one storage entry between two consecutive scans.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryChange {
    /// The entry's risk status changed.
    StatusChanged { from: RiskLevel, to: RiskLevel },
    /// `live_until_ledger` increased: the entry's TTL was bumped.
    Extended {
        from_live_until: u32,
        to_live_until: u32,
    },
    /// The entry is now on the ledger and was not in the previous scan.
    Appeared { status: RiskLevel },
    /// The entry was on the ledger in the previous scan and is now missing.
    Disappeared { last_status: RiskLevel },
}

/// A state transition of one storage entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transition {
    /// The entry's key identifier (e.g. `persistent:COUNTER`).
    pub entry: String,
    /// The entry's durability tier.
    pub durability: Durability,
    /// What changed.
    pub change: EntryChange,
}

/// An entry is on the ledger when the scan found it: missing entries report `live_until_ledger` 0.
fn on_ledger(entry: &StorageEntryReport) -> bool {
    entry.live_until_ledger > 0
}

fn transition(entry: &StorageEntryReport, change: EntryChange) -> Transition {
    Transition {
        entry: entry.key_identifier.clone(),
        durability: entry.durability,
        change,
    }
}

/// Compares consecutive reports and returns every entry transition, in the current report's
/// entry order followed by entries that are no longer reported.
pub fn detect_transitions(previous: &TTLReport, current: &TTLReport) -> Vec<Transition> {
    let find_previous = |id: &str| {
        previous
            .entries
            .iter()
            .find(|entry| entry.key_identifier == id)
    };
    let mut transitions = Vec::new();
    let mut seen = HashSet::new();

    for entry in &current.entries {
        if !seen.insert(entry.key_identifier.as_str()) {
            continue;
        }
        let before = find_previous(&entry.key_identifier).filter(|b| on_ledger(b));
        match (before, on_ledger(entry)) {
            (None, true) => transitions.push(transition(
                entry,
                EntryChange::Appeared {
                    status: entry.status,
                },
            )),
            (Some(before), false) => transitions.push(transition(
                entry,
                EntryChange::Disappeared {
                    last_status: before.status,
                },
            )),
            (Some(before), true) => {
                if before.status != entry.status {
                    transitions.push(transition(
                        entry,
                        EntryChange::StatusChanged {
                            from: before.status,
                            to: entry.status,
                        },
                    ));
                }
                if entry.live_until_ledger > before.live_until_ledger {
                    transitions.push(transition(
                        entry,
                        EntryChange::Extended {
                            from_live_until: before.live_until_ledger,
                            to_live_until: entry.live_until_ledger,
                        },
                    ));
                }
            }
            (None, false) => {}
        }
    }

    for before in previous.entries.iter().filter(|b| on_ledger(b)) {
        if seen.insert(before.key_identifier.as_str()) {
            transitions.push(transition(
                before,
                EntryChange::Disappeared {
                    last_status: before.status,
                },
            ));
        }
    }
    transitions
}

/// Formats a UNIX timestamp as RFC 3339 UTC (e.g. `2026-11-20T08:15:00Z`).
pub fn format_rfc3339_utc(unix_secs: u64) -> String {
    let days = (unix_secs / 86_400) as i64;
    let secs_of_day = unix_secs % 86_400;
    // Civil-from-days (Howard Hinnant), valid for the proleptic Gregorian calendar.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        year,
        month,
        day,
        secs_of_day / 3_600,
        (secs_of_day % 3_600) / 60,
        secs_of_day % 60
    )
}

fn now_rfc3339() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    format_rfc3339_utc(secs)
}

fn status_lower(status: RiskLevel) -> String {
    status.to_string().to_lowercase()
}

fn transition_text(transition: &Transition) -> String {
    let label = format_entry_description(&transition.entry);
    match transition.change {
        EntryChange::StatusChanged { from, to } => format!("{}: status {} -> {}", label, from, to),
        EntryChange::Extended {
            from_live_until,
            to_live_until,
        } => format!(
            "{}: live_until {} -> {} (+{} ledgers, bumped)",
            label,
            format_with_commas(from_live_until),
            format_with_commas(to_live_until),
            format_with_commas(u64::from(to_live_until - from_live_until))
        ),
        EntryChange::Appeared { status } => format!("{}: appeared ({})", label, status),
        EntryChange::Disappeared { last_status } => {
            format!("{}: disappeared (was {})", label, last_status)
        }
    }
}

fn transition_json(timestamp: &str, transition: &Transition) -> serde_json::Value {
    let mut value = json!({
        "type": "transition",
        "timestamp": timestamp,
        "entry": transition.entry,
        "durability": transition.durability.to_string().to_lowercase(),
    });
    let details = match transition.change {
        EntryChange::StatusChanged { from, to } => json!({
            "change": "status_changed",
            "from": status_lower(from),
            "to": status_lower(to),
        }),
        EntryChange::Extended {
            from_live_until,
            to_live_until,
        } => json!({
            "change": "extended",
            "from_live_until": from_live_until,
            "to_live_until": to_live_until,
        }),
        EntryChange::Appeared { status } => json!({
            "change": "appeared",
            "status": status_lower(status),
        }),
        EntryChange::Disappeared { last_status } => json!({
            "change": "disappeared",
            "last_status": status_lower(last_status),
        }),
    };
    if let (Some(target), serde_json::Value::Object(extra)) = (value.as_object_mut(), details) {
        target.extend(extra);
    }
    value
}

/// Loop settings for [`watch_loop`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WatchConfig {
    /// Time between scan starts.
    pub interval: Duration,
    /// Stop after this many scans (`None` runs until a shutdown signal).
    pub max_iterations: Option<u64>,
    /// Exit `11` after this many consecutive failed scans.
    pub max_consecutive_failures: u32,
    /// Output format.
    pub format: WatchFormat,
}

/// Writes the output of one successful scan: the full report first, then transitions or a
/// heartbeat. Write errors (e.g. a closed pipe) are ignored; the exit code still reflects risk.
fn emit_report<W: Write>(
    out: &mut W,
    format: WatchFormat,
    previous: Option<&TTLReport>,
    report: &TTLReport,
) {
    let timestamp = now_rfc3339();
    let lines: Vec<String> = match (previous, format) {
        (None, WatchFormat::Text) => vec![format!(
            "[{}] Initial scan\n{}",
            timestamp,
            render_text(report)
        )],
        (None, WatchFormat::Json) => vec![json!({
            "type": "report",
            "timestamp": timestamp,
            "report": JsonTTLReport::from(report),
        })
        .to_string()],
        (Some(previous), _) => {
            let transitions = detect_transitions(previous, report);
            match (transitions.is_empty(), format) {
                (true, WatchFormat::Text) => vec![format!(
                    "[{}] heartbeat: no changes (latest ledger {}, overall {})",
                    timestamp,
                    format_with_commas(report.latest_ledger),
                    report.overall_status
                )],
                (true, WatchFormat::Json) => vec![json!({
                    "type": "heartbeat",
                    "timestamp": timestamp,
                    "latest_ledger": report.latest_ledger,
                    "overall_status": status_lower(report.overall_status),
                })
                .to_string()],
                (false, WatchFormat::Text) => transitions
                    .iter()
                    .map(|t| format!("[{}] {}", timestamp, transition_text(t)))
                    .collect(),
                (false, WatchFormat::Json) => transitions
                    .iter()
                    .map(|t| transition_json(&timestamp, t).to_string())
                    .collect(),
            }
        }
    };
    for line in lines {
        let _ = writeln!(out, "{}", line);
    }
    let _ = out.flush();
}

/// Runs scans on `config.interval` until `max_iterations`, `shutdown`, a fatal error, or too many
/// consecutive RPC failures, and returns the exit code.
///
/// A transient RPC failure is logged to `err` and the loop continues. Invalid input exits `10`
/// immediately. Reaching `max_consecutive_failures` exits `11`. Otherwise the exit code is the
/// risk status of the last successful scan (`0` if no scan completed before shutdown, `11` if
/// every scan failed). `shutdown` also cancels an in-flight scan.
pub async fn watch_loop<R, W, E, S>(
    usecase: &ScanContractUseCase<R>,
    request: &ScanRequest,
    config: WatchConfig,
    out: &mut W,
    err: &mut E,
    shutdown: S,
) -> i32
where
    R: LedgerReader,
    W: Write,
    E: Write,
    S: Future<Output = ()>,
{
    let mut interval = tokio::time::interval(config.interval);
    interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
    tokio::pin!(shutdown);

    let mut last_report: Option<TTLReport> = None;
    let mut consecutive_failures = 0u32;
    let mut any_failure = false;
    let mut iterations = 0u64;

    loop {
        if config.max_iterations.is_some_and(|max| iterations >= max) {
            break;
        }
        tokio::select! {
            biased;
            _ = &mut shutdown => break,
            _ = interval.tick() => {}
        }
        iterations += 1;

        let result = tokio::select! {
            biased;
            _ = &mut shutdown => break,
            result = usecase.execute(request.clone()) => result,
        };

        match result {
            Ok(report) => {
                consecutive_failures = 0;
                if last_report.is_none() {
                    if let Some(note) = entry_size_limit_note(&report) {
                        let _ = writeln!(err, "{}", note);
                    }
                }
                emit_report(out, config.format, last_report.as_ref(), &report);
                last_report = Some(report);
            }
            Err(error) if is_rpc_failure(&error) => {
                consecutive_failures += 1;
                any_failure = true;
                let _ = writeln!(
                    err,
                    "[{}] scan failed ({}/{} consecutive): {}",
                    now_rfc3339(),
                    consecutive_failures,
                    config.max_consecutive_failures,
                    error
                );
                if consecutive_failures >= config.max_consecutive_failures {
                    let _ = writeln!(
                        err,
                        "Error: {} consecutive scans failed; stopping watch.",
                        consecutive_failures
                    );
                    return EXIT_RPC_FAILURE;
                }
            }
            Err(error) => {
                let _ = writeln!(err, "Error: {}", error);
                return exit_code_for_error(&error);
            }
        }
    }

    match last_report {
        Some(report) => exit_code_for_status(report.overall_status),
        None if any_failure => EXIT_RPC_FAILURE,
        None => EXIT_OK,
    }
}

/// Resolves on Ctrl-C, or on SIGTERM on Unix. If no handler can be installed it never resolves,
/// so the watch keeps running rather than stopping immediately.
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        if let Ok(mut terminate) = signal(SignalKind::terminate()) {
            tokio::select! {
                result = tokio::signal::ctrl_c() => {
                    if result.is_err() {
                        terminate.recv().await;
                    }
                }
                _ = terminate.recv() => {}
            }
            return;
        }
    }
    if tokio::signal::ctrl_c().await.is_err() {
        std::future::pending::<()>().await;
    }
}

/// Validates watch-only flags. Errors are user-facing messages for exit code `10`.
pub(crate) fn validate_watch_args(args: &WatchArgs) -> Result<(), String> {
    if !(MIN_WATCH_INTERVAL_SECS..=MAX_WATCH_INTERVAL_SECS).contains(&args.interval) {
        return Err(format!(
            "--interval must be between {} and {} seconds (got {})",
            MIN_WATCH_INTERVAL_SECS, MAX_WATCH_INTERVAL_SECS, args.interval
        ));
    }
    if !(1..=MAX_CONSECUTIVE_FAILURES_LIMIT).contains(&args.max_consecutive_failures) {
        return Err(format!(
            "--max-consecutive-failures must be between 1 and {} (got {})",
            MAX_CONSECUTIVE_FAILURES_LIMIT, args.max_consecutive_failures
        ));
    }
    if args.max_iterations == Some(0) {
        return Err("--max-iterations must be at least 1".to_string());
    }
    Ok(())
}

/// Executes the `sentinel watch` command and returns the appropriate UNIX exit code.
pub async fn run_watch(args: WatchArgs) -> i32 {
    if let Err(message) = validate_watch_args(&args) {
        eprintln!("Error: {}", message);
        return EXIT_INVALID_ARGS;
    }
    let (rpc_client, request) = match prepare_scan(&args.target) {
        Ok(prepared) => prepared,
        Err(message) => {
            eprintln!("Error: {}", message);
            return EXIT_INVALID_ARGS;
        }
    };
    let config = WatchConfig {
        interval: Duration::from_secs(args.interval),
        max_iterations: args.max_iterations,
        max_consecutive_failures: args.max_consecutive_failures,
        format: args.format,
    };
    let usecase = ScanContractUseCase::new(rpc_client);
    watch_loop(
        &usecase,
        &request,
        config,
        &mut std::io::stdout(),
        &mut std::io::stderr(),
        shutdown_signal(),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::Mutex;

    use async_trait::async_trait;
    use sentinel_core::adapters::derive_contract_instance_key;
    use sentinel_core::domain::EntryType;
    use sentinel_core::error::{SentinelError, SentinelResult};
    use sentinel_core::ports::{LatestLedgerInfo, RawLedgerEntry};
    use stellar_xdr::curr::{
        ContractDataDurability, ContractDataEntry, ContractExecutable, ExtensionPoint, Hash,
        LedgerEntryData, Limits, ScAddress, ScContractInstance, ScVal, WriteXdr,
    };

    const CONTRACT: [u8; 32] = [42u8; 32];

    fn entry(id: &str, live_until: u32, status: RiskLevel) -> StorageEntryReport {
        StorageEntryReport::new(
            id,
            EntryType::PersistentData,
            Durability::Persistent,
            64,
            live_until,
            0,
            0.0,
            0.0,
            status,
        )
    }

    fn report(entries: Vec<StorageEntryReport>) -> TTLReport {
        let overall_status = sentinel_core::compute_overall_status(&entries);
        TTLReport {
            contract_id: "CWATCH".to_string(),
            network: "testnet".to_string(),
            latest_ledger: 5_400_000,
            ledger_velocity_seconds: 5.0,
            entries,
            overall_status,
            archival_recommendations: Vec::new(),
        }
    }

    #[test]
    fn test_detect_transitions_none_when_unchanged() {
        let a = report(vec![entry("instance", 5_600_000, RiskLevel::Ok)]);
        assert!(detect_transitions(&a, &a.clone()).is_empty());
    }

    #[test]
    fn test_detect_transitions_status_change_and_bump() {
        let before = report(vec![
            entry("instance", 5_600_000, RiskLevel::Ok),
            entry("persistent:A", 5_450_000, RiskLevel::Warn),
            entry("persistent:B", 5_500_000, RiskLevel::Ok),
        ]);
        let after = report(vec![
            entry("instance", 5_600_000, RiskLevel::Warn),
            entry("persistent:A", 5_900_000, RiskLevel::Ok),
            entry("persistent:B", 5_500_000, RiskLevel::Ok),
        ]);
        assert_eq!(
            detect_transitions(&before, &after),
            vec![
                Transition {
                    entry: "instance".to_string(),
                    durability: Durability::Persistent,
                    change: EntryChange::StatusChanged {
                        from: RiskLevel::Ok,
                        to: RiskLevel::Warn
                    },
                },
                Transition {
                    entry: "persistent:A".to_string(),
                    durability: Durability::Persistent,
                    change: EntryChange::StatusChanged {
                        from: RiskLevel::Warn,
                        to: RiskLevel::Ok
                    },
                },
                Transition {
                    entry: "persistent:A".to_string(),
                    durability: Durability::Persistent,
                    change: EntryChange::Extended {
                        from_live_until: 5_450_000,
                        to_live_until: 5_900_000
                    },
                },
            ]
        );
    }

    #[test]
    fn test_detect_transitions_live_until_decrease_is_not_a_bump() {
        let before = report(vec![entry("persistent:A", 5_600_000, RiskLevel::Ok)]);
        let after = report(vec![entry("persistent:A", 5_599_000, RiskLevel::Ok)]);
        assert!(detect_transitions(&before, &after).is_empty());
    }

    #[test]
    fn test_detect_transitions_appeared_and_disappeared() {
        let before = report(vec![
            entry("instance", 5_600_000, RiskLevel::Ok),
            entry("persistent:Gone", 5_600_000, RiskLevel::Ok),
            entry("persistent:Restored", 0, RiskLevel::Archived),
            entry("code_wasm", 5_600_000, RiskLevel::Warn),
        ]);
        let after = report(vec![
            entry("instance", 5_600_000, RiskLevel::Ok),
            entry("persistent:Gone", 0, RiskLevel::Archived),
            entry("persistent:Restored", 5_700_000, RiskLevel::Ok),
            entry("persistent:New", 5_700_000, RiskLevel::Ok),
        ]);
        let changes: Vec<(String, EntryChange)> = detect_transitions(&before, &after)
            .into_iter()
            .map(|t| (t.entry, t.change))
            .collect();
        assert_eq!(
            changes,
            vec![
                (
                    "persistent:Gone".to_string(),
                    EntryChange::Disappeared {
                        last_status: RiskLevel::Ok
                    }
                ),
                (
                    "persistent:Restored".to_string(),
                    EntryChange::Appeared {
                        status: RiskLevel::Ok
                    }
                ),
                (
                    "persistent:New".to_string(),
                    EntryChange::Appeared {
                        status: RiskLevel::Ok
                    }
                ),
                (
                    "code_wasm".to_string(),
                    EntryChange::Disappeared {
                        last_status: RiskLevel::Warn
                    }
                ),
            ]
        );
    }

    #[test]
    fn test_detect_transitions_ignores_entries_missing_in_both_scans() {
        let missing = report(vec![entry("persistent:X", 0, RiskLevel::Archived)]);
        assert!(detect_transitions(&missing, &missing.clone()).is_empty());
    }

    #[test]
    fn test_format_rfc3339_utc() {
        assert_eq!(format_rfc3339_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(format_rfc3339_utc(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(format_rfc3339_utc(1_795_163_045), "2026-11-20T08:24:05Z");
    }

    /// Serves one scripted ledger state (or an RPC error) per scan, repeating the last one.
    struct ScriptedReader {
        steps: Mutex<VecDeque<SentinelResult<(u32, u32)>>>,
        current: Mutex<SentinelResult<(u32, u32)>>,
    }

    impl ScriptedReader {
        /// Each step is `(latest_ledger, instance live_until)` or an error.
        fn new(steps: Vec<SentinelResult<(u32, u32)>>) -> Self {
            Self {
                steps: Mutex::new(steps.into()),
                current: Mutex::new(Ok((0, 0))),
            }
        }
    }

    fn instance_xdr() -> String {
        LedgerEntryData::ContractData(ContractDataEntry {
            ext: ExtensionPoint::V0,
            contract: ScAddress::Contract(Hash(CONTRACT)),
            key: ScVal::LedgerKeyContractInstance,
            durability: ContractDataDurability::Persistent,
            val: ScVal::ContractInstance(ScContractInstance {
                executable: ContractExecutable::StellarAsset,
                storage: None,
            }),
        })
        .to_xdr_base64(Limits::none())
        .unwrap()
    }

    #[async_trait]
    impl LedgerReader for ScriptedReader {
        async fn get_latest_ledger(&self) -> SentinelResult<LatestLedgerInfo> {
            let mut steps = self.steps.lock().unwrap();
            let mut current = self.current.lock().unwrap();
            if let Some(next) = steps.pop_front() {
                *current = next;
            }
            current
                .clone()
                .map(|(latest, _)| LatestLedgerInfo::new(latest, 22))
        }

        async fn get_ledger_entries(
            &self,
            keys_xdr_base64: &[String],
        ) -> SentinelResult<Vec<RawLedgerEntry>> {
            let (_, live_until) = self.current.lock().unwrap().clone()?;
            let instance_key = derive_contract_instance_key(&CONTRACT).unwrap();
            Ok(keys_xdr_base64
                .iter()
                .filter(|key| **key == instance_key)
                .map(|key| RawLedgerEntry::new(key, instance_xdr(), live_until, 1))
                .collect())
        }
    }

    fn timeout() -> SentinelError {
        SentinelError::RpcConnectionTimeout("scripted".to_string())
    }

    fn config(max_iterations: Option<u64>, format: WatchFormat) -> WatchConfig {
        WatchConfig {
            interval: Duration::from_secs(60),
            max_iterations,
            max_consecutive_failures: 3,
            format,
        }
    }

    async fn run(
        steps: Vec<SentinelResult<(u32, u32)>>,
        config: WatchConfig,
        shutdown: impl Future<Output = ()>,
    ) -> (i32, String, String) {
        let usecase = ScanContractUseCase::new(ScriptedReader::new(steps));
        let request = ScanRequest::new(stellar_strkey::Contract(CONTRACT).to_string());
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = watch_loop(&usecase, &request, config, &mut out, &mut err, shutdown).await;
        (
            code,
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }

    #[tokio::test(start_paused = true)]
    async fn test_watch_prints_report_then_transitions_then_heartbeat() {
        // 200,000 left (OK) -> 100,000 left (WARN) -> bumped back to OK -> unchanged
        let started = tokio::time::Instant::now();
        let (code, out, err) = run(
            vec![
                Ok((5_400_000, 5_600_000)),
                Ok((5_500_000, 5_600_000)),
                Ok((5_500_000, 5_800_000)),
                Ok((5_500_000, 5_800_000)),
            ],
            config(Some(4), WatchFormat::Text),
            std::future::pending(),
        )
        .await;

        assert_eq!(code, EXIT_OK);
        // Ticks at 0s, 60s, 120s, 180s: no burst, no real sleeping (paused clock).
        assert_eq!(started.elapsed(), Duration::from_secs(180));
        // The scripted reader serves no entry-size limit: noted once, not on every scan.
        assert_eq!(
            err.matches("max_contract_data_entry_bytes limit could not be fetched")
                .count(),
            1,
            "{err}"
        );
        let lines: Vec<&str> = out.lines().collect();
        assert!(lines[0].ends_with("] Initial scan"));
        assert!(out.contains("Summary: 1 entries checked. 1 OK, 0 WARN, 0 CRITICAL/ARCHIVED."));
        let tail: Vec<&str> = lines
            .iter()
            .filter(|l| l.starts_with('['))
            .skip(1)
            .map(|l| l.split_once("] ").unwrap().1)
            .collect();
        assert_eq!(
            tail,
            vec![
                "Contract Instance: status OK -> WARN",
                "Contract Instance: status WARN -> OK",
                "Contract Instance: live_until 5,600,000 -> 5,800,000 (+200,000 ledgers, bumped)",
                "heartbeat: no changes (latest ledger 5,500,000, overall OK)",
            ]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn test_watch_json_emits_one_object_per_line() {
        let (code, out, _) = run(
            vec![
                Ok((5_400_000, 5_600_000)),
                Ok((5_500_000, 5_600_000)),
                Ok((5_500_000, 5_600_000)),
            ],
            config(Some(3), WatchFormat::Json),
            std::future::pending(),
        )
        .await;

        assert_eq!(code, 1);
        let objects: Vec<serde_json::Value> = out
            .lines()
            .map(|line| serde_json::from_str(line).expect("each line is one JSON object"))
            .collect();
        assert_eq!(objects.len(), 3);
        assert_eq!(objects[0]["type"], "report");
        assert_eq!(objects[0]["report"]["entries"][0]["identifier"], "instance");
        assert_eq!(objects[1]["type"], "transition");
        assert_eq!(objects[1]["change"], "status_changed");
        assert_eq!(objects[1]["entry"], "instance");
        assert_eq!(objects[1]["from"], "ok");
        assert_eq!(objects[1]["to"], "warn");
        assert_eq!(objects[2]["type"], "heartbeat");
        assert_eq!(objects[2]["overall_status"], "warn");
        assert!(objects.iter().all(|o| o["timestamp"].is_string()));
    }

    #[tokio::test(start_paused = true)]
    async fn test_watch_survives_transient_failures() {
        let (code, out, err) = run(
            vec![
                Ok((5_400_000, 5_600_000)),
                Err(timeout()),
                Err(timeout()),
                Ok((5_400_000, 5_600_000)),
            ],
            config(Some(4), WatchFormat::Text),
            std::future::pending(),
        )
        .await;

        assert_eq!(code, EXIT_OK);
        assert!(err.contains("scan failed (1/3 consecutive)"));
        assert!(err.contains("scan failed (2/3 consecutive)"));
        assert!(out.contains("heartbeat: no changes"));
    }

    #[tokio::test(start_paused = true)]
    async fn test_watch_exits_11_after_consecutive_failures() {
        let (code, _, err) = run(
            vec![Ok((5_400_000, 5_410_000)), Err(timeout())],
            config(None, WatchFormat::Text),
            std::future::pending(),
        )
        .await;

        assert_eq!(code, EXIT_RPC_FAILURE);
        assert!(err.contains("scan failed (3/3 consecutive)"));
        assert!(err.contains("3 consecutive scans failed; stopping watch."));
    }

    #[tokio::test(start_paused = true)]
    async fn test_watch_shutdown_exits_with_last_status() {
        // 10,000 ledgers left -> CRITICAL; shutdown between the second and third scans.
        let (code, out, _) = run(
            vec![Ok((5_400_000, 5_410_000))],
            config(None, WatchFormat::Text),
            tokio::time::sleep(Duration::from_secs(90)),
        )
        .await;

        assert_eq!(code, 2);
        assert!(out.contains("Initial scan"));
        assert_eq!(out.matches("heartbeat: no changes").count(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn test_watch_shutdown_before_any_scan_exits_ok() {
        let (code, out, _) = run(
            vec![Ok((5_400_000, 5_600_000))],
            config(None, WatchFormat::Text),
            std::future::ready(()),
        )
        .await;
        assert_eq!(code, EXIT_OK);
        assert!(out.is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn test_watch_invalid_input_is_fatal() {
        let usecase = ScanContractUseCase::new(ScriptedReader::new(vec![Ok((1, 1))]));
        let request = ScanRequest::new("NOT_A_CONTRACT");
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = watch_loop(
            &usecase,
            &request,
            config(None, WatchFormat::Text),
            &mut out,
            &mut err,
            std::future::pending(),
        )
        .await;
        assert_eq!(code, EXIT_INVALID_ARGS);
        assert!(String::from_utf8(err)
            .unwrap()
            .contains("Invalid contract address"));
    }

    fn watch_args(extra: &[&str]) -> WatchArgs {
        use clap::Parser;
        let mut argv = vec!["sentinel", "watch", "CABC"];
        argv.extend_from_slice(extra);
        match crate::args::Cli::parse_from(argv).command {
            crate::args::Commands::Watch(args) => args,
            _ => panic!("expected watch command"),
        }
    }

    #[test]
    fn test_validate_watch_args_bounds() {
        assert!(validate_watch_args(&watch_args(&[])).is_ok());
        assert!(validate_watch_args(&watch_args(&["--interval", "10"])).is_ok());
        assert!(validate_watch_args(&watch_args(&["--interval", "86400"])).is_ok());
        assert!(validate_watch_args(&watch_args(&["--interval", "9"])).is_err());
        assert!(validate_watch_args(&watch_args(&["--interval", "86401"])).is_err());
        assert!(validate_watch_args(&watch_args(&["--max-consecutive-failures", "0"])).is_err());
        assert!(validate_watch_args(&watch_args(&["--max-consecutive-failures", "101"])).is_err());
        assert!(validate_watch_args(&watch_args(&["--max-iterations", "0"])).is_err());
    }
}
