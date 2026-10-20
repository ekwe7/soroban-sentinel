//! ANSI-colorized ASCII table formatter for contract scan reports.

use comfy_table::presets::ASCII_FULL;
use comfy_table::{Cell, Color, Table};
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

    let header_line = format!(
        "Contract: {} | Network: {} | Latest Ledger: {} (Velocity: {:.1}s/ledger)",
        report.contract_id,
        report.network,
        format_with_commas(report.latest_ledger),
        report.ledger_velocity_seconds
    );

    format!("{}\n{}\n{}", header_line, table, summary_line)
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
        };

        let output = render_text(&report);

        assert!(output.contains("CCONTRACT123"));
        assert!(output.contains("ARCHIVED"));
        assert!(output.contains("CRITICAL"));
        assert!(output.contains("Summary: 2 entries checked. 0 OK, 0 WARN, 2 CRITICAL/ARCHIVED."));
    }
}
