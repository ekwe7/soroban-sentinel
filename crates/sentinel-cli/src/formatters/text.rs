//! ANSI-colorized ASCII table formatter for contract scan reports.

use std::ffi::OsStr;

use comfy_table::presets::ASCII_FULL;
use comfy_table::{Cell, Color, Table};
use sentinel_core::domain::compare::{BaselineDiff, ChangeCategory};
use sentinel_core::domain::RiskLevel;
use sentinel_core::usecases::{StorageEntryReport, TTLReport};

/// Returns `true` when the standard `NO_COLOR` environment variable is set to a non-empty value.
pub fn is_no_color_env() -> bool {
    std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty())
}

/// Evaluates whether ANSI color output should be suppressed given a `--no-color` flag
/// and an optional `NO_COLOR` environment variable value.
fn resolve_no_color(no_color_flag: bool, no_color_env: Option<&OsStr>) -> bool {
    no_color_flag || no_color_env.is_some_and(|v| !v.is_empty())
}

/// Returns `true` when ANSI color output should be suppressed via `--no-color` or `NO_COLOR`.
pub fn is_color_disabled(no_color_flag: bool) -> bool {
    resolve_no_color(no_color_flag, std::env::var_os("NO_COLOR").as_deref())
}

/// Formats an integer or numeric display type with standard thousands commas (e.g. 5,420,100).
pub fn format_with_commas<T: std::fmt::Display>(val: T) -> String {
    let s = val.to_string();
    let parts: Vec<&str> = s.split('.').collect();
    let int_part = parts[0];
    let is_negative = int_part.starts_with('-');
    let digits = if is_negative {
        &int_part[1..]
    } else {
        int_part
    };

    let mut result = String::new();
    let len = digits.len();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (len - i) % 3 == 0 {
            result.push(',');
        }
        result.push(c);
    }
    let mut formatted = if is_negative {
        format!("-{}", result)
    } else {
        result
    };
    if parts.len() > 1 {
        formatted.push('.');
        formatted.push_str(parts[1]);
    }
    formatted
}

/// Resolves a human-friendly display label for an entry identifier.
fn format_entry_description(key_identifier: &str) -> String {
    match key_identifier {
        "instance" => "Contract Instance".to_string(),
        "code_wasm" => "Contract Code (WASM)".to_string(),
        id if id.starts_with("persistent:") => {
            format!("Persistent Key: {}", id.trim_start_matches("persistent:"))
        }
        id if id.starts_with("persistent_xdr:") => {
            format!(
                "Persistent Key (XDR): {}",
                id.trim_start_matches("persistent_xdr:")
            )
        }
        other => other.to_string(),
    }
}

/// Maps a `RiskLevel` to a table `Cell`, optionally without ANSI foreground color.
fn format_status_cell(status: RiskLevel, no_color: bool) -> Cell {
    let cell = Cell::new(status.to_string());
    if no_color {
        return cell;
    }
    match status {
        RiskLevel::Ok => cell.fg(Color::Green),
        RiskLevel::Warn => cell.fg(Color::Yellow),
        RiskLevel::Critical => cell.fg(Color::Red),
        RiskLevel::Archived => cell.fg(Color::DarkGrey),
    }
}

/// Formats a single storage entry into a table row.
fn format_entry_row(entry: &StorageEntryReport, no_color: bool) -> Vec<Cell> {
    let description = format_entry_description(&entry.key_identifier);
    let durability = entry.durability.to_string();
    let size_bytes = format!("{} B", format_with_commas(entry.byte_size));
    let live_until = if entry.live_until_ledger == 0 {
        "0".to_string()
    } else {
        format_with_commas(entry.live_until_ledger)
    };

    let remaining_horizon = if entry.live_until_ledger == 0 || entry.remaining_ledgers <= 0 {
        "0 (0.0 d)".to_string()
    } else {
        format!(
            "{} (~{:.1} d)",
            format_with_commas(entry.remaining_ledgers),
            entry.remaining_days
        )
    };

    let status_cell = format_status_cell(entry.status, no_color);

    vec![
        Cell::new(description),
        Cell::new(durability),
        Cell::new(size_bytes),
        Cell::new(live_until),
        Cell::new(remaining_horizon),
        status_cell,
    ]
}

fn build_report_table(report: &TTLReport, no_color: bool) -> Table {
    let mut table = Table::new();
    table.load_preset(ASCII_FULL);
    if no_color {
        table.force_no_tty();
    }
    table.set_header(vec![
        "Entry Description",
        "Durability",
        "Size Bytes",
        "Live Until Ledger",
        "Remaining Horizon",
        "Status",
    ]);

    for entry in &report.entries {
        table.add_row(format_entry_row(entry, no_color));
    }

    table
}

/// Renders a `TTLReport` into an ergonomic ASCII table with summary health line.
///
/// Honors the standard `NO_COLOR` environment variable automatically.
pub fn render_text(report: &TTLReport) -> String {
    render_text_with_options(report, false)
}

/// Renders a `TTLReport` into an ASCII table, suppressing all ANSI escape sequences
/// when `no_color` is `true` or the `NO_COLOR` environment variable is set to a non-empty value.
pub fn render_text_with_options(report: &TTLReport, no_color: bool) -> String {
    let disable_color = is_color_disabled(no_color);
    let table = build_report_table(report, disable_color);

    let summary = report.summary();
    let crit_or_archived = summary.critical + summary.archived;
    let summary_line = format!(
        "Summary: {} entries checked. {} OK, {} WARN, {} CRITICAL/ARCHIVED.",
        summary.total_entries, summary.ok, summary.warn, crit_or_archived
    );

    let header_line = format!(
        "Contract: {} | Network: {} | Latest Ledger: {} (Velocity: {:.1}s/ledger)",
        report.contract_id,
        report.network,
        format_with_commas(report.latest_ledger),
        report.ledger_velocity_seconds
    );

    format!("{}\n{}\n{}", header_line, table, summary_line)
}

/// Formats a change category into a table cell, optionally without ANSI foreground color.
fn format_category_cell(cat: ChangeCategory, no_color: bool) -> Cell {
    let cell = Cell::new(cat.to_string());
    if no_color {
        return cell;
    }
    match cat {
        ChangeCategory::Extended | ChangeCategory::StatusImproved => cell.fg(Color::Green),
        ChangeCategory::Unchanged => cell,
        ChangeCategory::Decayed => cell.fg(Color::DarkGrey),
        ChangeCategory::StatusWorsened | ChangeCategory::Removed => cell.fg(Color::Red),
        ChangeCategory::Added => cell.fg(Color::Cyan),
    }
}

fn build_diff_table(diff: &BaselineDiff, no_color: bool) -> Table {
    let mut diff_table = Table::new();
    diff_table.load_preset(ASCII_FULL);
    if no_color {
        diff_table.force_no_tty();
    }
    diff_table.set_header(vec![
        "Entry Description",
        "Change Category",
        "Baseline Live-Until",
        "Current Live-Until",
        "Delta (Ledgers)",
        "Status Change",
    ]);

    for entry in &diff.entries {
        let desc = format_entry_description(&entry.identifier);
        let cat_cell = format_category_cell(entry.category, no_color);
        let b_live = if entry.baseline_live_until == 0 {
            "-".to_string()
        } else {
            format_with_commas(entry.baseline_live_until)
        };
        let c_live = if entry.current_live_until == 0 {
            "-".to_string()
        } else {
            format_with_commas(entry.current_live_until)
        };
        let delta_str = if entry.live_until_delta > 0 {
            format!("+{}", format_with_commas(entry.live_until_delta))
        } else if entry.live_until_delta < 0 {
            format_with_commas(entry.live_until_delta)
        } else {
            "0".to_string()
        };

        let status_change = match (entry.baseline_status, entry.current_status) {
            (Some(b), Some(c)) => {
                if b == c {
                    b.to_string()
                } else {
                    format!("{} -> {}", b, c)
                }
            }
            (None, Some(c)) => format!("(new) -> {}", c),
            (Some(b), None) => format!("{} -> (removed)", b),
            (None, None) => "-".to_string(),
        };

        diff_table.add_row(vec![
            Cell::new(desc),
            cat_cell,
            Cell::new(b_live),
            Cell::new(c_live),
            Cell::new(delta_str),
            Cell::new(status_change),
        ]);
    }

    diff_table
}

/// Renders a `TTLReport` alongside a `BaselineDiff` comparison.
///
/// Honors the standard `NO_COLOR` environment variable automatically.
pub fn render_text_with_diff(report: &TTLReport, diff: Option<&BaselineDiff>) -> String {
    render_text_with_diff_and_options(report, diff, false)
}

/// Renders a `TTLReport` alongside a `BaselineDiff` comparison, suppressing all ANSI
/// escape sequences when `no_color` is `true` or `NO_COLOR` is set to a non-empty value.
pub fn render_text_with_diff_and_options(
    report: &TTLReport,
    diff: Option<&BaselineDiff>,
    no_color: bool,
) -> String {
    let disable_color = is_color_disabled(no_color);
    let base_text = render_text_with_options(report, disable_color);
    let Some(d) = diff else {
        return base_text;
    };

    let diff_table = build_diff_table(d, disable_color);

    let elapsed_sign = if d.ledgers_elapsed >= 0 { "+" } else { "" };
    let summary_line = format!(
        "Baseline Diff Summary: {} extended, {} unchanged, {} decayed, {} worsened, {} improved, {} added, {} removed. (Elapsed: {}{} ledgers)",
        d.summary.extended,
        d.summary.unchanged,
        d.summary.decayed,
        d.summary.status_worsened,
        d.summary.status_improved,
        d.summary.added,
        d.summary.removed,
        elapsed_sign,
        format_with_commas(d.ledgers_elapsed),
    );

    format!(
        "{}\n\nBaseline Comparison (Baseline Ledger: {})\n{}\n{}",
        base_text,
        format_with_commas(d.baseline_latest_ledger),
        diff_table,
        summary_line
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use sentinel_core::domain::{Durability, EntryType};

    #[test]
    fn test_format_with_commas_helper() {
        assert_eq!(format_with_commas(0), "0");
        assert_eq!(format_with_commas(42), "42");
        assert_eq!(format_with_commas(999), "999");
        assert_eq!(format_with_commas(1000), "1,000");
        assert_eq!(format_with_commas(18450), "18,450");
        assert_eq!(format_with_commas(5420100), "5,420,100");
        assert_eq!(format_with_commas(-12345), "-12,345");
    }

    #[test]
    fn test_render_text_contains_headers_and_contract_id() {
        let contract_id = "CA3D5NXQ6DNG5BB25US67P3J6R74V4A7N5E55LFEV74J5R4R37E2L6G5";
        let entries = vec![
            StorageEntryReport::new(
                "instance",
                EntryType::ContractInstance,
                Durability::Instance,
                342,
                5_530_100,
                110_000,
                550_000.0,
                6.36,
                RiskLevel::Warn,
            ),
            StorageEntryReport::new(
                "code_wasm",
                EntryType::ContractCode,
                Durability::Code,
                18_450,
                5_530_100,
                110_000,
                550_000.0,
                6.36,
                RiskLevel::Warn,
            ),
            StorageEntryReport::new(
                "persistent:Admin",
                EntryType::PersistentData,
                Durability::Persistent,
                64,
                5_800_000,
                379_900,
                1_899_500.0,
                21.98,
                RiskLevel::Ok,
            ),
        ];

        let report = TTLReport {
            contract_id: contract_id.to_string(),
            network: "testnet".to_string(),
            latest_ledger: 5_420_100,
            ledger_velocity_seconds: 5.0,
            burn_rate: Default::default(),
            entries,
            overall_status: RiskLevel::Warn,
            archival_recommendations: Vec::new(),
            sac_asset_info: None,
            contract_code_hash: None,
            contract_bytecode_size: None,
            contract_upgrade: None,
        };

        let output = render_text(&report);

        // Verify contract ID and network metadata are in the output
        assert!(output.contains(contract_id));
        assert!(output.contains("testnet"));
        assert!(output.contains("5,420,100"));
        assert!(output.contains("Velocity: 5.0s/ledger"));

        // Verify column headers exist in output
        assert!(output.contains("Entry Description"));
        assert!(output.contains("Durability"));
        assert!(output.contains("Size Bytes"));
        assert!(output.contains("Live Until Ledger"));
        assert!(output.contains("Remaining Horizon"));
        assert!(output.contains("Status"));

        // Verify table content
        assert!(output.contains("Contract Instance"));
        assert!(output.contains("Contract Code (WASM)"));
        assert!(output.contains("Persistent Key: Admin"));
        assert!(output.contains("342 B"));
        assert!(output.contains("18,450 B"));
        assert!(output.contains("64 B"));

        // Verify bottom summary line
        assert!(output.contains("Summary: 3 entries checked. 1 OK, 2 WARN, 0 CRITICAL/ARCHIVED."));
    }

    #[test]
    fn test_render_text_with_archived_and_critical_entries() {
        let entries = vec![
            StorageEntryReport::new(
                "instance",
                EntryType::ContractInstance,
                Durability::Instance,
                0,
                0,
                0,
                0.0,
                0.0,
                RiskLevel::Archived,
            ),
            StorageEntryReport::new(
                "persistent:Balance",
                EntryType::PersistentData,
                Durability::Persistent,
                128,
                5_425_000,
                5_000,
                25_000.0,
                0.29,
                RiskLevel::Critical,
            ),
        ];

        let report = TTLReport {
            contract_id: "CCONTRACT123".to_string(),
            network: "mainnet".to_string(),
            latest_ledger: 5_420_000,
            ledger_velocity_seconds: 5.0,
            burn_rate: Default::default(),
            entries,
            overall_status: RiskLevel::Archived,
            archival_recommendations: Vec::new(),
            sac_asset_info: None,
            contract_code_hash: None,
            contract_bytecode_size: None,
            contract_upgrade: None,
        };

        let output = render_text(&report);

        assert!(output.contains("CCONTRACT123"));
        assert!(output.contains("ARCHIVED"));
        assert!(output.contains("CRITICAL"));
        assert!(output.contains("Summary: 2 entries checked. 0 OK, 0 WARN, 2 CRITICAL/ARCHIVED."));
    }

    #[test]
    fn test_resolve_no_color_honors_flag_and_non_empty_env() {
        assert!(!resolve_no_color(false, None));
        assert!(!resolve_no_color(false, Some(OsStr::new(""))));
        assert!(resolve_no_color(false, Some(OsStr::new("1"))));
        assert!(resolve_no_color(false, Some(OsStr::new("0"))));
        assert!(resolve_no_color(false, Some(OsStr::new("true"))));
        assert!(resolve_no_color(true, None));
        assert!(resolve_no_color(true, Some(OsStr::new(""))));
        assert!(resolve_no_color(true, Some(OsStr::new("1"))));
    }

    #[test]
    fn test_no_color_strips_all_ansi_escape_sequences_even_with_forced_styling() {
        use sentinel_core::domain::compare::{BaselineDiff, BaselineDiffSummary, EntryDiff};

        let entries = vec![
            StorageEntryReport::new(
                "instance",
                EntryType::ContractInstance,
                Durability::Instance,
                342,
                5_530_100,
                110_000,
                550_000.0,
                6.36,
                RiskLevel::Warn,
            ),
            StorageEntryReport::new(
                "persistent:Balance",
                EntryType::PersistentData,
                Durability::Persistent,
                128,
                5_425_000,
                5_000,
                25_000.0,
                0.29,
                RiskLevel::Critical,
            ),
            StorageEntryReport::new(
                "persistent:Admin",
                EntryType::PersistentData,
                Durability::Persistent,
                64,
                5_800_000,
                379_900,
                1_899_500.0,
                21.98,
                RiskLevel::Ok,
            ),
            StorageEntryReport::new(
                "persistent:Old",
                EntryType::PersistentData,
                Durability::Persistent,
                0,
                0,
                0,
                0.0,
                0.0,
                RiskLevel::Archived,
            ),
        ];

        let report = TTLReport {
            contract_id: "CCONTRACT123".to_string(),
            network: "testnet".to_string(),
            latest_ledger: 5_420_000,
            ledger_velocity_seconds: 5.0,
            burn_rate: Default::default(),
            entries,
            overall_status: RiskLevel::Archived,
            archival_recommendations: Vec::new(),
            sac_asset_info: None,
            contract_code_hash: None,
            contract_bytecode_size: None,
            contract_upgrade: None,
        };

        // Sanity check: when color is enabled and styling is enforced, ANSI codes ARE emitted.
        let mut colored_table = build_report_table(&report, false);
        colored_table.enforce_styling();
        assert!(colored_table.to_string().contains("\x1b["));

        // When no_color is active, even with enforce_styling() forced on the table, zero ANSI codes exist.
        let mut plain_table = build_report_table(&report, true);
        plain_table.enforce_styling();
        let plain_rendered = plain_table.to_string();
        assert!(!plain_rendered.contains("\x1b["));
        assert!(plain_rendered.contains("WARN"));
        assert!(plain_rendered.contains("CRITICAL"));
        assert!(plain_rendered.contains("OK"));
        assert!(plain_rendered.contains("ARCHIVED"));

        // Same check for baseline diff table.
        let diff = BaselineDiff {
            baseline_latest_ledger: 5_300_000,
            current_latest_ledger: 5_420_000,
            ledgers_elapsed: 120_000,
            entries: vec![
                EntryDiff {
                    identifier: "instance".to_string(),
                    category: ChangeCategory::StatusWorsened,
                    baseline_live_until: 5_530_100,
                    current_live_until: 5_530_100,
                    live_until_delta: 0,
                    baseline_status: Some(RiskLevel::Ok),
                    current_status: Some(RiskLevel::Warn),
                },
                EntryDiff {
                    identifier: "persistent:Admin".to_string(),
                    category: ChangeCategory::Extended,
                    baseline_live_until: 5_500_000,
                    current_live_until: 5_800_000,
                    live_until_delta: 300_000,
                    baseline_status: Some(RiskLevel::Ok),
                    current_status: Some(RiskLevel::Ok),
                },
            ],
            summary: BaselineDiffSummary {
                extended: 1,
                unchanged: 0,
                decayed: 0,
                status_worsened: 1,
                status_improved: 0,
                added: 0,
                removed: 0,
                has_regression: true,
            },
        };

        let mut colored_diff_table = build_diff_table(&diff, false);
        colored_diff_table.enforce_styling();
        assert!(colored_diff_table.to_string().contains("\x1b["));

        let mut plain_diff_table = build_diff_table(&diff, true);
        plain_diff_table.enforce_styling();
        assert!(!plain_diff_table.to_string().contains("\x1b["));

        let full_plain = render_text_with_diff_and_options(&report, Some(&diff), true);
        assert!(!full_plain.contains("\x1b["));
        assert!(full_plain.contains("Baseline Comparison"));
    }
}
