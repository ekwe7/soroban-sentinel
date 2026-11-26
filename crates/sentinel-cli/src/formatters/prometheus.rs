//! Prometheus text exposition formatter for the node_exporter textfile collector.
//!
//! Output is deterministic: families and samples are emitted in a fixed order, entries keep the
//! scan's report order, and no timestamps are written.

use std::collections::HashSet;
use std::fmt::Write as _;

use sentinel_core::domain::RiskLevel;
use sentinel_core::usecases::{StorageEntryReport, TTLReport};

/// Escapes a Prometheus label value (`\\`, `\"` and `\n`).
pub fn escape_label_value(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\n' => escaped.push_str("\\n"),
            other => escaped.push(other),
        }
    }
    escaped
}

/// Numeric encoding of `soroban_sentinel_entry_status`.
pub fn status_value(status: RiskLevel) -> u8 {
    match status {
        RiskLevel::Ok => 0,
        RiskLevel::Warn => 1,
        RiskLevel::Critical => 2,
        RiskLevel::Archived => 3,
    }
}

type EntryValue = fn(&StorageEntryReport) -> String;

const ENTRY_FAMILIES: [(&str, &str, EntryValue); 4] = [
    (
        "soroban_sentinel_entry_live_until_ledger",
        "Ledger sequence until which the storage entry stays live (0 when missing or archived).",
        |e| e.live_until_ledger.to_string(),
    ),
    (
        "soroban_sentinel_entry_remaining_ledgers",
        "Ledgers remaining before the storage entry expires.",
        |e| e.remaining_ledgers.to_string(),
    ),
    (
        "soroban_sentinel_entry_status",
        "Storage entry risk status: 0=ok, 1=warn, 2=critical, 3=archived.",
        |e| status_value(e.status).to_string(),
    ),
    (
        "soroban_sentinel_entry_size_bytes",
        "Serialized size of the storage entry in bytes.",
        |e| e.byte_size.to_string(),
    ),
];

fn write_family_header(out: &mut String, name: &str, help: &str) {
    let _ = writeln!(out, "# HELP {} {}", name, help);
    let _ = writeln!(out, "# TYPE {} gauge", name);
}

/// Renders a `TTLReport` in the Prometheus text exposition format.
pub fn render_prometheus(report: &TTLReport) -> String {
    let contract = escape_label_value(&report.contract_id);
    let network = escape_label_value(&report.network);

    // A key passed twice would otherwise produce duplicate series, which Prometheus rejects.
    let mut seen = HashSet::new();
    let entries: Vec<(&StorageEntryReport, String, String)> = report
        .entries
        .iter()
        .filter(|e| seen.insert((e.key_identifier.as_str(), e.durability)))
        .map(|e| {
            (
                e,
                escape_label_value(&e.key_identifier),
                e.durability.to_string().to_lowercase(),
            )
        })
        .collect();

    let mut out = String::new();
    write_family_header(
        &mut out,
        "soroban_sentinel_latest_ledger",
        "Latest ledger sequence observed by the scan.",
    );
    let _ = writeln!(
        out,
        "soroban_sentinel_latest_ledger{{contract=\"{}\",network=\"{}\"}} {}",
        contract, network, report.latest_ledger
    );

    for (name, help, value) in ENTRY_FAMILIES {
        write_family_header(&mut out, name, help);
        for (entry, entry_label, durability) in &entries {
            let _ = writeln!(
                out,
                "{}{{contract=\"{}\",network=\"{}\",entry=\"{}\",durability=\"{}\"}} {}",
                name,
                contract,
                network,
                entry_label,
                durability,
                value(entry)
            );
        }
    }

    // `println!` adds the final newline that the exposition format requires.
    out.truncate(out.trim_end_matches('\n').len());
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use sentinel_core::domain::{Durability, EntryType};

    fn entry(
        id: &str,
        durability: Durability,
        live_until: u32,
        status: RiskLevel,
    ) -> StorageEntryReport {
        StorageEntryReport::new(
            id,
            EntryType::PersistentData,
            durability,
            64,
            live_until,
            i64::from(live_until) - 5_400_000,
            0.0,
            0.0,
            status,
        )
    }

    fn report(entries: Vec<StorageEntryReport>) -> TTLReport {
        TTLReport {
            contract_id: "CCONTRACT".to_string(),
            network: "testnet".to_string(),
            latest_ledger: 5_400_000,
            ledger_velocity_seconds: 5.0,
            entries,
            overall_status: RiskLevel::Ok,
            archival_recommendations: Vec::new(),
            ..TTLReport::default()
        }
    }

    #[test]
    fn test_render_prometheus_exposition_format() {
        let output = render_prometheus(&report(vec![
            entry("instance", Durability::Instance, 5_600_000, RiskLevel::Ok),
            entry(
                "persistent:Admin",
                Durability::Persistent,
                5_450_000,
                RiskLevel::Warn,
            ),
        ]));

        assert!(output.starts_with(
            "# HELP soroban_sentinel_latest_ledger Latest ledger sequence observed by the scan.\n# TYPE soroban_sentinel_latest_ledger gauge\nsoroban_sentinel_latest_ledger{contract=\"CCONTRACT\",network=\"testnet\"} 5400000\n"
        ));
        for family in [
            "soroban_sentinel_latest_ledger",
            "soroban_sentinel_entry_live_until_ledger",
            "soroban_sentinel_entry_remaining_ledgers",
            "soroban_sentinel_entry_status",
            "soroban_sentinel_entry_size_bytes",
        ] {
            assert_eq!(
                output.matches(&format!("# HELP {} ", family)).count(),
                1,
                "{family}"
            );
            assert_eq!(
                output
                    .matches(&format!("# TYPE {} gauge\n", family))
                    .count(),
                1,
                "{family}"
            );
        }
        assert!(output.contains(
            "soroban_sentinel_entry_live_until_ledger{contract=\"CCONTRACT\",network=\"testnet\",entry=\"instance\",durability=\"instance\"} 5600000"
        ));
        assert!(output.contains(
            "soroban_sentinel_entry_remaining_ledgers{contract=\"CCONTRACT\",network=\"testnet\",entry=\"persistent:Admin\",durability=\"persistent\"} 50000"
        ));
        assert!(output.contains(
            "soroban_sentinel_entry_status{contract=\"CCONTRACT\",network=\"testnet\",entry=\"persistent:Admin\",durability=\"persistent\"} 1"
        ));
        assert!(!output.ends_with('\n'));

        // Every sample is `name{labels} value`: no timestamp column.
        for line in output.lines().filter(|l| !l.starts_with('#')) {
            let after_labels = line.rsplit_once("} ").expect("sample with labels").1;
            assert!(after_labels.parse::<i64>().is_ok(), "{line}");
        }
    }

    #[test]
    fn test_status_value_encoding() {
        assert_eq!(status_value(RiskLevel::Ok), 0);
        assert_eq!(status_value(RiskLevel::Warn), 1);
        assert_eq!(status_value(RiskLevel::Critical), 2);
        assert_eq!(status_value(RiskLevel::Archived), 3);
    }

    #[test]
    fn test_render_prometheus_escapes_hostile_label_values() {
        let mut hostile = report(vec![entry(
            "persistent:a\"b\\c\nd} 1",
            Durability::Persistent,
            5_600_000,
            RiskLevel::Ok,
        )]);
        hostile.network = "net\"work".to_string();
        let output = render_prometheus(&hostile);

        assert!(output.contains(
            "soroban_sentinel_entry_status{contract=\"CCONTRACT\",network=\"net\\\"work\",entry=\"persistent:a\\\"b\\\\c\\nd} 1\",durability=\"persistent\"} 0"
        ));
        // The raw newline never reaches the output, so each sample stays on one line.
        assert_eq!(
            output.lines().filter(|l| !l.starts_with('#')).count(),
            1 + ENTRY_FAMILIES.len()
        );
    }

    #[test]
    fn test_render_prometheus_is_deterministic_and_deduplicated() {
        let entries = vec![
            entry(
                "persistent:B",
                Durability::Persistent,
                5_600_000,
                RiskLevel::Ok,
            ),
            entry(
                "persistent:A",
                Durability::Persistent,
                5_600_000,
                RiskLevel::Ok,
            ),
            entry(
                "persistent:B",
                Durability::Persistent,
                5_600_000,
                RiskLevel::Ok,
            ),
            entry(
                "temporary:B",
                Durability::Temporary,
                5_600_000,
                RiskLevel::Ok,
            ),
        ];
        let first = render_prometheus(&report(entries.clone()));
        assert_eq!(first, render_prometheus(&report(entries)));

        let status_entries: Vec<&str> = first
            .lines()
            .filter(|l| l.starts_with("soroban_sentinel_entry_status{"))
            .map(|l| {
                l.split("entry=\"")
                    .nth(1)
                    .unwrap()
                    .split('"')
                    .next()
                    .unwrap()
            })
            .collect();
        assert_eq!(
            status_entries,
            vec!["persistent:B", "persistent:A", "temporary:B"]
        );
    }

    #[test]
    fn test_escape_label_value() {
        assert_eq!(escape_label_value("plain"), "plain");
        assert_eq!(escape_label_value("a\\b"), "a\\\\b");
        assert_eq!(escape_label_value("a\"b"), "a\\\"b");
        assert_eq!(escape_label_value("a\nb"), "a\\nb");
    }
}
