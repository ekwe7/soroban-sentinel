//! OpenMetrics text exposition for one-shot scan jobs.

use sentinel_core::{Durability, EntryType, RiskLevel, TTLReport};

const REMAINING: &str = "soroban_sentinel_entry_remaining_ledgers";
const SIZE: &str = "soroban_sentinel_entry_byte_size";
const RISK: &str = "soroban_sentinel_entry_risk_level";

/// Render a scan as an OpenMetrics-compatible payload suitable for Pushgateway.
pub fn render_pushgateway(report: &TTLReport) -> String {
    let mut output = String::new();
    metric_header(
        &mut output,
        REMAINING,
        "Remaining ledger count before an entry expires.",
    );
    metric_header(&mut output, SIZE, "Serialized ledger entry size in bytes.");
    metric_header(
        &mut output,
        RISK,
        "Entry risk level: OK=0, WARN=1, CRITICAL=2, ARCHIVED=3.",
    );

    for entry in &report.entries {
        let labels = format!(
            "contract_id=\"{}\",durability=\"{}\",entry_type=\"{}\",network=\"{}\"",
            escape_label(&report.contract_id),
            durability_label(entry.durability),
            entry_type_label(entry.entry_type),
            escape_label(&report.network),
        );
        output.push_str(&format!(
            "{REMAINING}{{{labels}}} {}\n",
            entry.remaining_ledgers
        ));
        output.push_str(&format!("{SIZE}{{{labels}}} {}\n", entry.byte_size));
        output.push_str(&format!(
            "{RISK}{{{labels}}} {}\n",
            risk_value(entry.status)
        ));
    }
    output.push_str("# EOF\n");
    output
}

fn metric_header(output: &mut String, name: &str, help: &str) {
    output.push_str(&format!("# HELP {name} {help}\n# TYPE {name} gauge\n"));
}

fn escape_label(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace(['\n', '\r'], "\\n")
}

fn durability_label(value: Durability) -> &'static str {
    match value {
        Durability::Instance => "instance",
        Durability::Persistent => "persistent",
        Durability::Temporary => "temporary",
        Durability::Code => "code",
    }
}

fn entry_type_label(value: EntryType) -> &'static str {
    match value {
        EntryType::ContractInstance => "contract_instance",
        EntryType::ContractCode => "contract_code",
        EntryType::PersistentData => "contract_data",
        EntryType::TemporaryData => "temporary_data",
    }
}

fn risk_value(value: RiskLevel) -> u8 {
    match value {
        RiskLevel::Ok => 0,
        RiskLevel::Warn => 1,
        RiskLevel::Critical => 2,
        RiskLevel::Archived => 3,
    }
}

#[cfg(test)]
mod tests {
    use super::escape_label;

    #[test]
    fn label_escaping_handles_quotes_backslashes_and_line_breaks() {
        assert_eq!(escape_label("a\\b\"c\nd\re"), "a\\\\b\\\"c\\nd\\ne");
    }
}
