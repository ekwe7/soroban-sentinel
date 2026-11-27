//! JUnit XML report formatter for CI/CD test-runner integration.

use sentinel_core::domain::compare::BaselineDiff;
use sentinel_core::domain::RiskLevel;
use sentinel_core::usecases::{StorageEntryReport, TTLReport};

/// XML-escapes a value for inclusion in JUnit XML text content.
fn xml_escape(s: &str) -> String {
    s.replace("&", "&amp;")
        .replace("<", "&lt;")
        .replace(">", "&gt;")
        .replace("\"", "&quot;")
        .replace("'", "&apos;")
}

/// Converts a `RiskLevel` into a JUnit-compatible status string.
fn status_from_risk(risk: RiskLevel) -> &'static str {
    match risk {
        RiskLevel::Ok => "passed",
        RiskLevel::Warn => "warning",
        RiskLevel::Critical => "failed",
        RiskLevel::Archived => "failed",
    }
}

/// A single JUnit `<testcase>` resulted from scanning one storage entry.
struct EntryTestCase {
    name: String,
    classname: String,
    status: &'static str,
    message: String,
    live_until_ledger: u32,
    remaining_ledgers: i64,
    remaining_days: f64,
    byte_size: usize,
}

impl EntryTestCase {
    fn from_entry(entry: &StorageEntryReport, contract_id: &str) -> Self {
        let classname = format!("{}.{}", contract_id, entry.durability);
        let name = entry.key_identifier.clone();
        let (status, message) = if entry.live_until_ledger == 0 {
            (
                "failed",
                format!("Storage entry '{}' is archived (live_until_ledger=0).", xml_escape(&entry.key_identifier)),
            )
        } else if entry.remaining_ledgers <= 0 {
            (
                "failed",
                format!(
                    "Storage entry '{}' expired at ledger {} (current ledger higher).",
                    xml_escape(&entry.key_identifier),
                    entry.live_until_ledger
                ),
            )
        } else {
            let status = status_from_risk(entry.status);
            let message = match entry.status {
                RiskLevel::Warn => format!(
                    "Storage entry '{}' is near expiry: {} ledgers (~{:.1} days) remaining until ledger {}.",
                    xml_escape(&entry.key_identifier),
                    entry.remaining_ledgers,
                    entry.remaining_days,
                    entry.live_until_ledger
                ),
                RiskLevel::Critical => format!(
                    "Storage entry '{}' is at critical risk: {} ledgers (~{:.1} days) remaining until ledger {}.",
                    xml_escape(&entry.key_identifier),
                    entry.remaining_ledgers,
                    entry.remaining_days,
                    entry.live_until_ledger
                ),
                RiskLevel::Archived => format!(
                    "Storage entry '{}' is archived: live_until_ledger={}.",
                    xml_escape(&entry.key_identifier),
                    entry.live_until_ledger
                ),
                RiskLevel::Ok => String::new(),
            };
            (status, message)
        };

        Self {
            name,
            classname,
            status,
            message,
            live_until_ledger: entry.live_until_ledger,
            remaining_ledgers: entry.remaining_ledgers,
            remaining_days: entry.remaining_days,
            byte_size: entry.byte_size,
        }
    }
}

/// Renders a `TTLReport` as a JUnit XML document.
///
/// Each storage entry becomes one `<testcase>` with a nested failure/warning element
/// when its projected status is not `OK`. A baseline diff is reflected as
/// additional properties on affected testcases.
pub fn render_junit(report: &TTLReport, diff: Option<&BaselineDiff>) -> String {
    let contract = &report.contract_id;
    let mut out = String::with_capacity(2048);
    out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    out.push_str("<testsuites>\n");
    out.push_str(&format!(
        "  <testsuite name=\"sentinel.scan\" tests=\"{}\" failures=\"{}\" warnings=\"{}\">\n",
        report.entries.len(),
        report
            .entries
            .iter()
            .filter(|e| e.status == RiskLevel::Critical || e.status == RiskLevel::Archived)
            .count(),
        report
            .entries
            .iter()
            .filter(|e| e.status == RiskLevel::Warn)
            .count(),
    ));

    for entry in &report.entries {
        let tc = EntryTestCase::from_entry(entry, contract);
        let diff_for_entry: Option<EntryDiffFromBaseline> = diff.and_then(|d| {
            d.entries
                .iter()
                .find(|ed| ed.identifier == tc.name)
                .map(|ed| EntryDiffFromBaseline {
                    category: format!("{}", ed.category),
                    baseline_live_until: ed.baseline_live_until,
                    live_until_delta: ed.live_until_delta,
                })
        });

        out.push_str("    <testcase ");
        out.push_str(&format!("name=\"{}\" ", xml_escape(&tc.name)));
        out.push_str(&format!("classname=\"{}\" ", xml_escape(&tc.classname)));
        out.push_str(&format!(
            "status=\"{}\" ",
            tc.status
        ));
        out.push_str(&format!(
            "live_until_ledger=\"{}\" ",
            tc.live_until_ledger
        ));
        out.push_str(&format!(
            "remaining_ledgers=\"{}\" ",
            tc.remaining_ledgers
        ));
        out.push_str(&format!(
            "remaining_days=\"{:.2}\" ",
            tc.remaining_days
        ));
        out.push_str(&format!("byte_size=\"{}\" ", tc.byte_size));
        if let Some(ref d) = diff_for_entry {
            out.push_str(&format!(
                "baseline_live_until=\"{}\" ",
                d.baseline_live_until
            ));
            out.push_str(&format!(
                "live_until_delta=\"{}\" ",
                d.live_until_delta
            ));
            out.push_str(&format!(
                "category=\"{}\" ",
                xml_escape(&d.category)
            ));
        }
        out.push('>');

        if tc.status == "failed" {
            out.push_str("\n      <failure>");
            out.push_str(&xml_escape(&tc.message));
            out.push_str("</failure>\n    ");
        }

        out.push_str("</testcase>\n");
    }

    out.push_str("  </testsuite>\n");
    out.push_str("</testsuites>\n");
    out
}

/// Lightweight view of a baseline diff entry used by the JUnit renderer.
struct EntryDiffFromBaseline {
    category: String,
    baseline_live_until: u32,
    live_until_delta: i64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use sentinel_core::domain::{Durability, EntryType};

    fn entry(
        key_identifier: &str,
        durability: Durability,
        byte_size: usize,
        live_until_ledger: u32,
        remaining_ledgers: i64,
        remaining_days: f64,
        status: RiskLevel,
    ) -> StorageEntryReport {
        StorageEntryReport {
            key_identifier: key_identifier.to_string(),
            entry_type: EntryType::PersistentData,
            durability,
            byte_size,
            live_until_ledger,
            remaining_ledgers,
            estimated_remaining_seconds: remaining_days * 86400.0,
            remaining_days,
            status,
            instance_storage_keys: Vec::new(),
        }
    }

    fn basic_report(entries: Vec<StorageEntryReport>) -> TTLReport {
        TTLReport {
            contract_id: "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM".to_string(),
            network: "testnet".to_string(),
            latest_ledger: 5_420_100,
            ledger_velocity_seconds: 5.0,
            burn_rate: Default::default(),
            entries,
            overall_status: RiskLevel::Ok,
            archival_recommendations: Vec::new(),
            sac_asset_info: None,
            contract_code_hash: None,
            contract_bytecode_size: None,
            contract_upgrade: None,
        }
    }

    #[test]
    fn test_render_junit_escapes_xml_characters() {
        let entries = vec![entry(
            "persistent:Key<&>\"'",
            Durability::Persistent,
            128,
            5_500_000,
            80_000,
            0.92,
            RiskLevel::Warn,
        )];
        let report = basic_report(entries);
        let rendered = render_junit(&report, None);

        // Should not contain raw XML metacharacters in the message.
        assert!(!rendered.contains("<&>"));
        assert!(rendered.contains("&lt;"));
        assert!(rendered.contains("&amp;"));
        assert!(rendered.contains("&quot;"));
        assert!(rendered.contains("&apos;"));

        // Should still be valid-ish XML structure.
        assert!(rendered.starts_with("<?xml"));
        assert!(rendered.contains("<testsuites>"));
        assert!(rendered.contains("<testsuite"));
        assert!(rendered.contains("<testcase"));
    }

    #[test]
    fn test_render_junit_ok_entry_has_no_failure_element() {
        let entries = vec![entry(
            "persistent:Stable",
            Durability::Persistent,
            64,
            5_800_000,
            380_000,
            4.4,
            RiskLevel::Ok,
        )];
        let report = basic_report(entries);
        let rendered = render_junit(&report, None);

        assert!(rendered.contains("status=\"passed\""));
        assert!(!rendered.contains("<failure>"));
        assert!(!rendered.contains("<warning>"));
        assert!(rendered.contains("tests=\"1\" failures=\"0\" warnings=\"0\""));
    }

    #[test]
    fn test_render_junit_critical_entry_has_failure() {
        let entries = vec![entry(
            "persistent:Balance",
            Durability::Persistent,
            128,
            5_425_000,
            5_000,
            0.06,
            RiskLevel::Critical,
        )];
        let report = basic_report(entries);
        let rendered = render_junit(&report, None);

        assert!(rendered.contains("status=\"failed\""));
        assert!(rendered.contains("<failure>"));
        assert!(rendered.contains("at critical risk"));
        assert!(rendered.contains("tests=\"1\" failures=\"1\" warnings=\"0\""));
    }

    #[test]
    fn test_render_junit_archived_entry_has_failure() {
        let entries = vec![entry(
            "persistent:Old",
            Durability::Persistent,
            0,
            0,
            0,
            0.0,
            RiskLevel::Archived,
        )];
        let report = basic_report(entries);
        let rendered = render_junit(&report, None);

        assert!(rendered.contains("status=\"failed\""));
        assert!(rendered.contains("<failure>"));
        assert!(rendered.contains("is archived"));
    }

    #[test]
    fn test_render_junit_includes_live_until_and_remaining_fields() {
        let entries = vec![entry(
            "persistent:Admin",
            Durability::Persistent,
            64,
            5_800_000,
            379_900,
            4.4,
            RiskLevel::Ok,
        )];
        let report = basic_report(entries);
        let rendered = render_junit(&report, None);

        assert!(rendered.contains("live_until_ledger=\"5800000\""));
        assert!(rendered.contains("remaining_ledgers=\"379900\""));
        assert!(rendered.contains("remaining_days=\"4.40\""));
        assert!(rendered.contains("byte_size=\"64\""));
    }

    #[test]
    fn test_render_junit_emits_classname_with_durability() {
        let entries = vec![
            entry(
                "instance",
                Durability::Instance,
                342,
                5_530_100,
                110_000,
                6.36,
                RiskLevel::Warn,
            ),
            entry(
                "persistent:Admin",
                Durability::Persistent,
                64,
                5_800_000,
                379_900,
                4.4,
                RiskLevel::Ok,
            ),
        ];
        let report = basic_report(entries);
        let rendered = render_junit(&report, None);

        assert!(rendered.contains("classname=\"CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM.Instance\""));
        assert!(rendered.contains("classname=\"CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM.Persistent\""));
    }
}
