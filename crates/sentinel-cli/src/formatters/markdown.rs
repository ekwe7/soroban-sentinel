//! Markdown scan report formatter for CI summaries.

use sentinel_core::usecases::TTLReport;

pub fn render_markdown(report: &TTLReport) -> String {
    let mut out = String::from("# Soroban Sentinel TTL Report\n\n");
    out.push_str(&format!(
        "- Contract: `{}`\n- Network: `{}`\n- Latest ledger: {}\n- Overall status: **{}**\n\n",
        report.contract_id, report.network, report.latest_ledger, report.overall_status
    ));
    out.push_str("| Entry | Durability | Remaining ledgers | Status |\n|---|---:|---:|---|\n");
    for entry in &report.entries {
        out.push_str(&format!(
            "| {} | {} | {} | {} |\n",
            entry.key_identifier.replace('|', "\\|"),
            entry.durability,
            entry.remaining_ledgers,
            entry.status
        ));
    }
    out
}
