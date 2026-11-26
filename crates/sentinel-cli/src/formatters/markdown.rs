//! GitHub-flavoured Markdown formatter for contract scan reports (PR and issue comments).

use sentinel_core::usecases::{StorageEntryReport, TTLReport};

use super::text::{entry_notes, format_entry_description, format_with_commas};

/// Escapes text for use in GFM table cells and inline prose.
///
/// Backslash-escapes the punctuation that can open inline markup, close a table cell (`|`) or
/// start a code span (`` ` ``), and folds line breaks into spaces so one value cannot break the
/// table row it sits in.
pub fn escape_markdown(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '\\' | '`' | '|' | '*' | '_' | '[' | ']' | '<' | '>' | '~' | '&' | '#' | '!' => {
                escaped.push('\\');
                escaped.push(c);
            }
            '\r' | '\n' => escaped.push(' '),
            other => escaped.push(other),
        }
    }
    escaped
}

fn format_entry_row(entry: &StorageEntryReport) -> String {
    let live_until = format_with_commas(entry.live_until_ledger);
    let remaining = if entry.live_until_ledger == 0 || entry.remaining_ledgers <= 0 {
        "0 (0.0 d)".to_string()
    } else {
        format!(
            "{} (~{:.1} d)",
            format_with_commas(entry.remaining_ledgers),
            entry.remaining_days
        )
    };
    format!(
        "| {} | {} | {} B | {} | {} | {} |",
        escape_markdown(&format_entry_description(&entry.key_identifier)),
        entry.durability,
        format_with_commas(entry.byte_size),
        live_until,
        remaining,
        entry.status
    )
}

/// Renders a `TTLReport` as a Markdown header, GFM table and summary line, with no ANSI codes.
pub fn render_markdown(report: &TTLReport) -> String {
    let mut lines = vec![
        "### Soroban Sentinel scan".to_string(),
        String::new(),
        format!(
            "**Contract:** {} · **Network:** {} · **Latest ledger:** {}",
            escape_markdown(&report.contract_id),
            escape_markdown(&report.network),
            format_with_commas(report.latest_ledger)
        ),
        String::new(),
        "| Entry | Durability | Size | Live Until | Remaining | Status |".to_string(),
        "| --- | --- | ---: | ---: | ---: | --- |".to_string(),
    ];
    lines.extend(report.entries.iter().map(format_entry_row));

    let summary = report.summary();
    lines.push(String::new());
    lines.push(format!(
        "**Summary:** {} entries checked. {} OK, {} WARN, {} CRITICAL/ARCHIVED.",
        summary.total_entries,
        summary.ok,
        summary.warn,
        summary.critical + summary.archived
    ));

    let notes: Vec<String> = report
        .entries
        .iter()
        .flat_map(entry_notes)
        .map(|note| format!("- {}", escape_markdown(&note)))
        .collect();
    if !notes.is_empty() {
        lines.push(String::new());
        lines.extend(notes);
    }

    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use sentinel_core::domain::{Durability, EntryType, RiskLevel};

    fn entry(id: &str, durability: Durability, status: RiskLevel) -> StorageEntryReport {
        let entry_type = match durability {
            Durability::Instance => EntryType::ContractInstance,
            Durability::Code => EntryType::ContractCode,
            Durability::Persistent => EntryType::PersistentData,
            Durability::Temporary => EntryType::TemporaryData,
        };
        let (live_until, remaining) = if status == RiskLevel::Archived {
            (0, 0)
        } else {
            (5_500_000, 100_000)
        };
        StorageEntryReport::new(
            id,
            entry_type,
            durability,
            1_234,
            live_until,
            remaining,
            remaining as f64 * 5.0,
            remaining as f64 * 5.0 / 86_400.0,
            status,
        )
    }

    fn report(entries: Vec<StorageEntryReport>) -> TTLReport {
        let overall_status = sentinel_core::compute_overall_status(&entries);
        TTLReport {
            contract_id: "CCONTRACT".to_string(),
            network: "testnet".to_string(),
            latest_ledger: 5_400_000,
            ledger_velocity_seconds: 5.0,
            entries,
            overall_status,
            archival_recommendations: Vec::new(),
        }
    }

    #[test]
    fn test_render_markdown_header_table_and_summary() {
        let output = render_markdown(&report(vec![
            entry("instance", Durability::Instance, RiskLevel::Ok),
            entry("persistent:Admin", Durability::Persistent, RiskLevel::Warn),
        ]));

        assert!(output.contains(
            "**Contract:** CCONTRACT · **Network:** testnet · **Latest ledger:** 5,400,000"
        ));
        assert!(output.contains("| Entry | Durability | Size | Live Until | Remaining | Status |"));
        assert!(output.contains("| --- | --- | ---: | ---: | ---: | --- |"));
        assert!(output.contains(
            "| Contract Instance | Instance | 1,234 B | 5,500,000 | 100,000 (~5.8 d) | OK |"
        ));
        assert!(output.contains("| Persistent Key: Admin | Persistent | 1,234 B |"));
        assert!(
            output.contains("**Summary:** 2 entries checked. 1 OK, 1 WARN, 0 CRITICAL/ARCHIVED.")
        );
        assert!(
            !output.contains('\x1b'),
            "markdown must not contain ANSI codes"
        );
    }

    #[test]
    fn test_render_markdown_escapes_hostile_identifiers() {
        let hostile = "persistent:a|b`c\n| injected | row |<script>*x*";
        let output = render_markdown(&report(vec![entry(
            hostile,
            Durability::Persistent,
            RiskLevel::Ok,
        )]));

        let row = output
            .lines()
            .find(|line| line.starts_with("| Persistent Key:"))
            .expect("entry row present");
        assert_eq!(
            row,
            "| Persistent Key: a\\|b\\`c \\| injected \\| row \\|\\<script\\>\\*x\\* | Persistent | 1,234 B | 5,500,000 | 100,000 (~5.8 d) | OK |"
        );
        // The newline was folded, so no extra table row was injected.
        assert!(!output.lines().any(|line| line.starts_with("| injected")));
        // Every unescaped pipe in the row is a column separator: 7 for 6 columns.
        let unescaped_pipes = row
            .char_indices()
            .filter(|&(i, c)| c == '|' && (i == 0 || row.as_bytes()[i - 1] != b'\\'))
            .count();
        assert_eq!(unescaped_pipes, 7);
    }

    #[test]
    fn test_render_markdown_escapes_header_values_and_notes() {
        let mut hostile = report(vec![entry(
            "temporary:NONCE`|",
            Durability::Temporary,
            RiskLevel::Archived,
        )]);
        hostile.network = "test|net`".to_string();
        let output = render_markdown(&hostile);

        assert!(output.contains("**Network:** test\\|net\\`"));
        assert!(
            output.contains("- UNRECOVERABLE: Temporary Key: NONCE\\`\\| is missing or expired.")
        );
    }

    #[test]
    fn test_escape_markdown_handles_backslash_and_line_breaks() {
        assert_eq!(escape_markdown("a\\b"), "a\\\\b");
        assert_eq!(escape_markdown("line1\r\nline2"), "line1  line2");
        assert_eq!(escape_markdown("plain-text 123"), "plain-text 123");
    }
}
