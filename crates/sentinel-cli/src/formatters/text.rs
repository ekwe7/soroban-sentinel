//! ANSI-colorized ASCII table formatter for contract scan reports.

use comfy_table::presets::ASCII_FULL;
use comfy_table::{Cell, Color, Table};
use sentinel_core::domain::compare::{BaselineDiff, ChangeCategory};
use sentinel_core::domain::RiskLevel;
use sentinel_core::usecases::{StorageEntryReport, TTLReport};

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
pub(crate) fn format_entry_description(key_identifier: &str) -> String {
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
        id if id.starts_with("temporary:") => {
            format!("Temporary Key: {}", id.trim_start_matches("temporary:"))
        }
        id if id.starts_with("temporary_xdr:") => {
            format!(
                "Temporary Key (XDR): {}",
                id.trim_start_matches("temporary_xdr:")
            )
        }
        other => other.to_string(),
    }
}

/// Maps a `RiskLevel` to a colored `Cell`.
fn format_status_cell(status: RiskLevel) -> Cell {
    match status {
        RiskLevel::Ok => Cell::new(status.to_string()).fg(Color::Green),
        RiskLevel::Warn => Cell::new(status.to_string()).fg(Color::Yellow),
        RiskLevel::Critical => Cell::new(status.to_string()).fg(Color::Red),
        RiskLevel::Archived => Cell::new(status.to_string()).fg(Color::DarkGrey),
    }
}

/// Formats a single storage entry into a table row.
fn format_entry_row(entry: &StorageEntryReport) -> Vec<Cell> {
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

    let status_cell = format_status_cell(entry.status);

    vec![
        Cell::new(description),
        Cell::new(durability),
        Cell::new(size_bytes),
        Cell::new(live_until),
        Cell::new(remaining_horizon),
        status_cell,
    ]
}

/// Renders a `TTLReport` into an ergonomic ASCII table with summary health line.
pub fn render_text(report: &TTLReport) -> String {
    let mut table = Table::new();
    table.load_preset(ASCII_FULL);
    table.set_header(vec![
        "Entry Description",
        "Durability",
        "Size Bytes",
        "Live Until Ledger",
        "Remaining Horizon",
        "Status",
    ]);

    for entry in &report.entries {
        table.add_row(format_entry_row(entry));
    }

    let summary = report.summary();
    let crit_or_archived = summary.critical + summary.archived;
    let summary_line = format!(
        "Summary: {} entries checked. {} OK, {} WARN, {} CRITICAL/ARCHIVED.",
        summary.total_entries, summary.ok, summary.warn, crit_or_archived
    );
    let notes: String = report
        .entries
        .iter()
        .flat_map(entry_notes)
        .map(|note| format!("\n{}", note))
        .collect();

    let header_line = format!(
        "Contract: {} | Network: {} | Latest Ledger: {} (Velocity: {:.1}s/ledger)",
        report.contract_id,
        report.network,
        format_with_commas(report.latest_ledger),
        report.ledger_velocity_seconds
    );

    format!("{}\n{}\n{}{}", header_line, table, summary_line, notes)
}

/// Explains what the table alone cannot convey: an unrecoverable temporary entry, and the
/// instance-storage size metrics (including when size, not TTL, raised the status).
pub(crate) fn entry_notes(entry: &StorageEntryReport) -> Vec<String> {
    let mut notes = Vec::new();
    if entry.is_unrecoverable() {
        notes.push(format!(
            "UNRECOVERABLE: {} is missing or expired. Temporary entries are deleted permanently and cannot be restored.",
            format_entry_description(&entry.key_identifier)
        ));
    }
    if let Some(metrics) = &entry.instance_storage {
        let size = match (metrics.max_entry_size_bytes, metrics.utilization_pct) {
            (Some(limit), Some(pct)) => format!(
                "{} B of the {} B network entry-size limit ({:.1}%)",
                format_with_commas(metrics.entry_size_bytes),
                format_with_commas(limit),
                pct
            ),
            _ => format!(
                "{} B (network entry-size limit unavailable; utilisation omitted)",
                format_with_commas(metrics.entry_size_bytes)
            ),
        };
        notes.push(format!(
            "Instance storage: {} keys, {}.",
            format_with_commas(metrics.key_count),
            size
        ));
        if metrics.size_status != RiskLevel::Ok {
            notes.push(format!(
                "{} (instance storage SIZE): the instance entry is approaching the network's maximum contract-data entry size, so storage writes may start failing. This status is caused by entry size, not TTL.",
                metrics.size_status
            ));
        }
    }
    notes
}

/// Formats a change category into a styled table cell.
fn format_category_cell(cat: ChangeCategory) -> Cell {
    match cat {
        ChangeCategory::Extended | ChangeCategory::StatusImproved => {
            Cell::new(cat.to_string()).fg(Color::Green)
        }
        ChangeCategory::Unchanged => Cell::new(cat.to_string()),
        ChangeCategory::Decayed => Cell::new(cat.to_string()).fg(Color::DarkGrey),
        ChangeCategory::StatusWorsened | ChangeCategory::Removed => {
            Cell::new(cat.to_string()).fg(Color::Red)
        }
        ChangeCategory::Added => Cell::new(cat.to_string()).fg(Color::Cyan),
    }
}

/// Renders a `TTLReport` alongside a `BaselineDiff` comparison.
pub fn render_text_with_diff(report: &TTLReport, diff: Option<&BaselineDiff>) -> String {
    let base_text = render_text(report);
    let Some(d) = diff else {
        return base_text;
    };

    let mut diff_table = Table::new();
    diff_table.load_preset(ASCII_FULL);
    diff_table.set_header(vec![
        "Entry Description",
        "Change Category",
        "Baseline Live-Until",
        "Current Live-Until",
        "Delta (Ledgers)",
        "Status Change",
    ]);

    for entry in &d.entries {
        let desc = format_entry_description(&entry.identifier);
        let cat_cell = format_category_cell(entry.category);
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
            entries,
            overall_status: RiskLevel::Warn,
            archival_recommendations: Vec::new(),
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
            entries,
            overall_status: RiskLevel::Archived,
            archival_recommendations: Vec::new(),
        };

        let output = render_text(&report);

        assert!(output.contains("CCONTRACT123"));
        assert!(output.contains("ARCHIVED"));
        assert!(output.contains("CRITICAL"));
        assert!(output.contains("Summary: 2 entries checked. 0 OK, 0 WARN, 2 CRITICAL/ARCHIVED."));
    }

    #[test]
    fn test_render_text_labels_temporary_entries_and_flags_unrecoverable() {
        let report = TTLReport {
            contract_id: "CTEMP".to_string(),
            network: "testnet".to_string(),
            latest_ledger: 5_400_000,
            ledger_velocity_seconds: 5.0,
            entries: vec![
                StorageEntryReport::new(
                    "temporary:NONCE",
                    EntryType::TemporaryData,
                    Durability::Temporary,
                    48,
                    5_450_000,
                    50_000,
                    250_000.0,
                    2.89,
                    RiskLevel::Warn,
                ),
                StorageEntryReport::new(
                    "temporary_xdr:AAAAAw==",
                    EntryType::TemporaryData,
                    Durability::Temporary,
                    0,
                    0,
                    0,
                    0.0,
                    0.0,
                    RiskLevel::Archived,
                ),
                StorageEntryReport::new(
                    "persistent:Gone",
                    EntryType::PersistentData,
                    Durability::Persistent,
                    0,
                    0,
                    0,
                    0.0,
                    0.0,
                    RiskLevel::Archived,
                ),
            ],
            overall_status: RiskLevel::Archived,
            archival_recommendations: Vec::new(),
        };

        let output = render_text(&report);
        assert!(output.contains("Temporary Key: NONCE"));
        assert!(output.contains("Temporary Key (XDR): AAAAAw=="));
        assert!(output.contains("Temporary"));
        assert!(output.contains(
            "UNRECOVERABLE: Temporary Key (XDR): AAAAAw== is missing or expired. Temporary entries are deleted permanently and cannot be restored."
        ));
        // A live temporary entry and an archived persistent entry are not unrecoverable.
        assert!(!output.contains("UNRECOVERABLE: Temporary Key: NONCE"));
        assert!(!output.contains("UNRECOVERABLE: Persistent Key: Gone"));
    }

    fn instance_with_metrics(
        status: RiskLevel,
        metrics: sentinel_core::usecases::InstanceStorageReport,
    ) -> TTLReport {
        let mut entry = StorageEntryReport::new(
            "instance",
            EntryType::ContractInstance,
            Durability::Instance,
            52_000,
            5_900_000,
            500_000,
            2_500_000.0,
            28.9,
            status,
        );
        entry.instance_storage = Some(metrics);
        TTLReport {
            contract_id: "CSIZE".to_string(),
            network: "testnet".to_string(),
            latest_ledger: 5_400_000,
            ledger_velocity_seconds: 5.0,
            entries: vec![entry],
            overall_status: status,
            archival_recommendations: Vec::new(),
        }
    }

    #[test]
    fn test_render_text_instance_storage_size_warning_names_size_as_cause() {
        let output = render_text(&instance_with_metrics(
            RiskLevel::Warn,
            sentinel_core::usecases::InstanceStorageReport {
                key_count: 60,
                entry_size_bytes: 52_429,
                max_entry_size_bytes: Some(65_536),
                utilization_pct: Some(80.0),
                size_status: RiskLevel::Warn,
            },
        ));
        assert!(output.contains(
            "Instance storage: 60 keys, 52,429 B of the 65,536 B network entry-size limit (80.0%)."
        ));
        assert!(output.contains("WARN (instance storage SIZE)"));
        assert!(output.contains("This status is caused by entry size, not TTL."));
    }

    #[test]
    fn test_render_text_instance_storage_without_limit() {
        let output = render_text(&instance_with_metrics(
            RiskLevel::Ok,
            sentinel_core::usecases::InstanceStorageReport {
                key_count: 3,
                entry_size_bytes: 400,
                max_entry_size_bytes: None,
                utilization_pct: None,
                size_status: RiskLevel::Ok,
            },
        ));
        assert!(output.contains(
            "Instance storage: 3 keys, 400 B (network entry-size limit unavailable; utilisation omitted)."
        ));
        assert!(!output.contains("instance storage SIZE"));
    }
}
