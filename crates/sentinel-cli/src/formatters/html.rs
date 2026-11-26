//! Self-contained HTML5 scan report formatter for offline audit artifacts.
//!
//! Every presentation rule lives in the `<style>` element in `<head>`. The
//! document contains no scripts and references no external assets, so it can
//! be archived or attached to CI artifacts and viewed without a network.

use std::time::{SystemTime, UNIX_EPOCH};

use sentinel_core::{RiskLevel, TTLReport};

const STYLES: &str = r#"
    :root { color-scheme: light; }
    * { box-sizing: border-box; }
    body {
      margin: 0;
      background: #f4f6f8;
      color: #1f2933;
      font: 14px/1.5 -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, Helvetica, Arial, sans-serif;
    }
    .report { max-width: 1120px; margin: 0 auto; padding: 32px 24px 48px; }
    .report-header { margin-bottom: 24px; }
    h1 { margin: 0 0 4px; font-size: 26px; }
    h2 {
      margin: 0 0 12px;
      font-size: 13px;
      letter-spacing: .06em;
      text-transform: uppercase;
      color: #52606d;
    }
    .subtitle { margin: 0 0 12px; color: #616e7c; }
    section {
      background: #fff;
      border: 1px solid #d9e2ec;
      border-radius: 8px;
      padding: 20px;
      margin-bottom: 20px;
    }
    dl {
      margin: 0;
      display: grid;
      grid-template-columns: repeat(auto-fit, minmax(220px, 1fr));
      gap: 12px 24px;
    }
    .meta-row { min-width: 0; }
    dt {
      font-size: 11px;
      font-weight: 700;
      letter-spacing: .05em;
      text-transform: uppercase;
      color: #52606d;
    }
    dd { margin: 2px 0 0; overflow-wrap: anywhere; }
    .mono { font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace; }
    .badge {
      display: inline-block;
      padding: 2px 10px;
      border-radius: 999px;
      border: 1px solid transparent;
      font-size: 11px;
      font-weight: 700;
      letter-spacing: .04em;
      text-transform: uppercase;
      white-space: nowrap;
    }
    .badge-ok { background: #d1fae5; color: #065f46; border-color: #6ee7b7; }
    .badge-warn { background: #fef3c7; color: #92400e; border-color: #fcd34d; }
    .badge-critical { background: #fee2e2; color: #991b1b; border-color: #fca5a5; }
    .badge-archived { background: #e2e8f0; color: #334155; border-color: #94a3b8; }
    .summary-badges {
      list-style: none;
      margin: 0;
      padding: 0;
      display: flex;
      flex-wrap: wrap;
      gap: 8px;
    }
    table { width: 100%; border-collapse: collapse; }
    caption { padding-bottom: 10px; text-align: left; font-size: 12px; color: #616e7c; }
    th, td {
      padding: 9px 10px;
      border-bottom: 1px solid #e4e9ef;
      text-align: left;
      vertical-align: middle;
    }
    thead th {
      background: #f0f4f8;
      font-size: 11px;
      letter-spacing: .04em;
      text-transform: uppercase;
      color: #334e68;
    }
    tbody tr:last-child td { border-bottom: 0; }
    .num { text-align: right; font-variant-numeric: tabular-nums; }
    .entry-id { font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace; }
    .countdown {
      min-width: 120px;
      height: 10px;
      background: #e4e9ef;
      border-radius: 999px;
      overflow: hidden;
    }
    .countdown-fill { height: 100%; border-radius: inherit; }
    .level-ok { background: #10b981; }
    .level-warn { background: #f59e0b; }
    .level-critical { background: #ef4444; }
    .level-archived { background: #64748b; }
    .countdown-note {
      display: block;
      margin-top: 4px;
      font-size: 11px;
      color: #616e7c;
    }
    .empty { margin: 0; font-style: italic; color: #616e7c; }
    .report-footer { text-align: center; font-size: 11px; color: #829ab1; }
    @media print {
      body { background: #fff; }
      section { break-inside: avoid; }
    }
"#;

/// Renders a scan report as a complete, self-contained HTML5 document.
pub fn render_html(report: &TTLReport) -> String {
    let timestamp = format_epoch_seconds(unix_timestamp_now());
    let overall_class = status_class(report.overall_status);
    let max_remaining = report
        .entries
        .iter()
        .map(|entry| entry.remaining_ledgers.max(0))
        .max()
        .unwrap_or(0);
    let summary = report.summary();

    let mut out = String::new();
    out.push_str("<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n");
    out.push_str("<meta charset=\"utf-8\">\n");
    out.push_str("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n");
    out.push_str("<title>Soroban Sentinel TTL Report</title>\n<style>\n");
    out.push_str(STYLES);
    for percent in 0..=100 {
        out.push_str(&format!(".bar-width-{0} {{ width: {0}%; }}\n", percent));
    }
    out.push_str("</style>\n</head>\n<body>\n<main class=\"report\">\n");

    out.push_str("<header class=\"report-header\">\n");
    out.push_str("<h1>Soroban Sentinel TTL Report</h1>\n");
    out.push_str("<p class=\"subtitle\">Contract TTL and archival risk audit</p>\n");
    out.push_str(&format!(
        "<p>Overall status: <span class=\"badge badge-{0}\">{1}</span></p>\n",
        overall_class,
        escape_html(&report.overall_status.to_string())
    ));
    out.push_str("</header>\n");

    out.push_str("<section class=\"metadata\" aria-labelledby=\"metadata-heading\">\n");
    out.push_str("<h2 id=\"metadata-heading\">Scan Metadata</h2>\n<dl>\n");
    out.push_str(&format!(
        "<div class=\"meta-row\"><dt>Contract ID</dt><dd class=\"mono\">{0}</dd></div>\n",
        escape_html(&report.contract_id)
    ));
    out.push_str(&format!(
        "<div class=\"meta-row\"><dt>Network</dt><dd>{0}</dd></div>\n",
        escape_html(&report.network)
    ));
    out.push_str(&format!(
        "<div class=\"meta-row\"><dt>Scan Timestamp</dt><dd><time datetime=\"{0}\">{0}</time></dd></div>\n",
        timestamp
    ));
    out.push_str(&format!(
        "<div class=\"meta-row\"><dt>Current Ledger Sequence</dt><dd class=\"num\">{0}</dd></div>\n",
        report.latest_ledger
    ));
    out.push_str(&format!(
        "<div class=\"meta-row\"><dt>Ledger Velocity</dt><dd>{0:.1}s / ledger</dd></div>\n",
        report.ledger_velocity_seconds
    ));
    out.push_str(&format!(
        "<div class=\"meta-row\"><dt>Overall Status</dt><dd><span class=\"badge badge-{0}\">{1}</span></dd></div>\n",
        overall_class,
        escape_html(&report.overall_status.to_string())
    ));
    out.push_str("</dl>\n</section>\n");

    out.push_str("<section class=\"summary\" aria-labelledby=\"summary-heading\">\n");
    out.push_str("<h2 id=\"summary-heading\">Entry Summary</h2>\n");
    out.push_str(&format!(
        "<p>{0} storage entries checked.</p>\n<ul class=\"summary-badges\">\n",
        summary.total_entries
    ));
    out.push_str(&format!(
        "<li><span class=\"badge badge-ok\">{0} OK</span></li>\n",
        summary.ok
    ));
    out.push_str(&format!(
        "<li><span class=\"badge badge-warn\">{0} WARN</span></li>\n",
        summary.warn
    ));
    out.push_str(&format!(
        "<li><span class=\"badge badge-critical\">{0} CRITICAL</span></li>\n",
        summary.critical
    ));
    out.push_str(&format!(
        "<li><span class=\"badge badge-archived\">{0} ARCHIVED</span></li>\n",
        summary.archived
    ));
    out.push_str("</ul>\n</section>\n");

    out.push_str("<section class=\"entries\" aria-labelledby=\"entries-heading\">\n");
    out.push_str("<h2 id=\"entries-heading\">Storage Entries</h2>\n");
    if report.entries.is_empty() {
        out.push_str("<p class=\"empty\">No storage entries were reported.</p>\n");
    } else {
        out.push_str("<table>\n");
        out.push_str(&format!(
            "<caption>TTL state at ledger {0}</caption>\n",
            report.latest_ledger
        ));
        out.push_str(
            "<thead><tr><th scope=\"col\">Entry</th><th scope=\"col\">Type</th><th scope=\"col\">Durability</th><th scope=\"col\">Size (bytes)</th><th scope=\"col\">Live Until Ledger</th><th scope=\"col\">Remaining Ledgers</th><th scope=\"col\">Remaining (days)</th><th scope=\"col\">Countdown</th><th scope=\"col\">Status</th></tr></thead>\n<tbody>\n",
        );
        for entry in &report.entries {
            let identifier = escape_html(&entry.key_identifier);
            let class = status_class(entry.status);
            let status = escape_html(&entry.status.to_string());
            let percent = countdown_percent(entry.remaining_ledgers, max_remaining);
            out.push_str("<tr>\n");
            out.push_str(&format!("  <td class=\"entry-id\">{0}</td>\n", identifier));
            out.push_str(&format!(
                "  <td>{0}</td>\n",
                escape_html(&entry.entry_type.to_string())
            ));
            out.push_str(&format!(
                "  <td>{0}</td>\n",
                escape_html(&entry.durability.to_string())
            ));
            out.push_str(&format!("  <td class=\"num\">{0}</td>\n", entry.byte_size));
            out.push_str(&format!(
                "  <td class=\"num\">{0}</td>\n",
                entry.live_until_ledger
            ));
            out.push_str(&format!(
                "  <td class=\"num\">{0}</td>\n",
                entry.remaining_ledgers
            ));
            out.push_str(&format!(
                "  <td class=\"num\">{0:.1}</td>\n",
                entry.remaining_days
            ));
            out.push_str(&format!(
                "  <td>\n    <div class=\"countdown\" role=\"meter\" aria-label=\"TTL countdown for {0}\" aria-valuemin=\"0\" aria-valuemax=\"100\" aria-valuenow=\"{1}\" data-ledgers=\"{2}\">\n      <div class=\"countdown-fill level-{3} bar-width-{1}\"></div>\n    </div>\n    <span class=\"countdown-note\">{1}% of longest remaining horizon</span>\n  </td>\n",
                identifier, percent, entry.remaining_ledgers, class
            ));
            out.push_str(&format!(
                "  <td><span class=\"badge badge-{0}\">{1}</span></td>\n",
                class, status
            ));
            out.push_str("</tr>\n");
        }
        out.push_str("</tbody>\n</table>\n");
    }
    out.push_str("</section>\n");
    out.push_str("<footer class=\"report-footer\">Generated by Soroban Sentinel &middot; Self-contained offline report</footer>\n");
    out.push_str("</main>\n</body>\n</html>\n");
    out
}

fn status_class(status: RiskLevel) -> &'static str {
    match status {
        RiskLevel::Ok => "ok",
        RiskLevel::Warn => "warn",
        RiskLevel::Critical => "critical",
        RiskLevel::Archived => "archived",
    }
}

fn countdown_percent(remaining_ledgers: i64, max_remaining: i64) -> u64 {
    if max_remaining <= 0 || remaining_ledgers <= 0 {
        return 0;
    }
    let ratio = remaining_ledgers as f64 / max_remaining as f64;
    (ratio * 100.0).round().clamp(0.0, 100.0) as u64
}

fn escape_html(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            _ => escaped.push(ch),
        }
    }
    escaped
}

fn unix_timestamp_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

/// Formats Unix seconds as `YYYY-MM-DDTHH:MM:SSZ` (UTC, ISO 8601).
fn format_epoch_seconds(seconds: i64) -> String {
    let days = seconds.div_euclid(86_400);
    let within_day = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_date_from_days(days);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        year,
        month,
        day,
        within_day / 3_600,
        (within_day % 3_600) / 60,
        within_day % 60
    )
}

/// Howard Hinnant's civil-from-days algorithm, valid for all Unix-era dates.
fn civil_date_from_days(days_since_epoch: i64) -> (i64, i64, i64) {
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    (year + if month <= 2 { 1 } else { 0 }, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sentinel_core::domain::{Durability, EntryType};
    use sentinel_core::usecases::StorageEntryReport;

    fn sample_report() -> TTLReport {
        TTLReport {
            contract_id: "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".to_string(),
            network: "testnet".to_string(),
            latest_ledger: 5_400_000,
            ledger_velocity_seconds: 5.0,
            entries: vec![
                StorageEntryReport::new(
                    "instance",
                    EntryType::ContractInstance,
                    Durability::Instance,
                    342,
                    5_500_000,
                    100_000,
                    500_000.0,
                    5.79,
                    RiskLevel::Warn,
                ),
                StorageEntryReport::new(
                    "persistent:Admin",
                    EntryType::PersistentData,
                    Durability::Persistent,
                    64,
                    5_800_000,
                    400_000,
                    2_000_000.0,
                    23.15,
                    RiskLevel::Ok,
                ),
            ],
            overall_status: RiskLevel::Warn,
            archival_recommendations: Vec::new(),
            sac_asset_info: None,
        }
    }

    #[test]
    fn renders_self_contained_html5_document() {
        let html = render_html(&sample_report());
        assert!(html.starts_with("<!DOCTYPE html>\n<html lang=\"en\">"));
        assert!(html.contains("<meta charset=\"utf-8\">"));
        assert!(html.contains("<style>\n"));
        assert!(html.contains("</style>\n</head>"));
        assert!(html.trim_end().ends_with("</html>"));
        assert!(!html.contains("<script"));
        assert!(!html.contains("http://"));
        assert!(!html.contains("https://"));

        for label in [
            "Contract ID",
            "Network",
            "Scan Timestamp",
            "Current Ledger Sequence",
        ] {
            assert!(html.contains(label), "missing metadata label: {}", label);
        }
        assert!(html.contains("badge-warn"));
        assert!(html.contains("badge-ok"));
        assert!(html.contains("class=\"countdown\""));
        assert!(html.contains("bar-width-100"));
        assert!(html.contains("bar-width-25"));
        assert!(html.contains("<td class=\"entry-id\">instance</td>"));
        assert!(html.contains("<td class=\"entry-id\">persistent:Admin</td>"));
    }

    #[test]
    fn escapes_html_in_dynamic_fields() {
        let mut report = sample_report();
        report.network = "<script>alert('x')</script>".to_string();
        report.entries[0].key_identifier = "<img src=x onerror=alert(1)>".to_string();
        let html = render_html(&report);
        assert!(html.contains("&lt;script&gt;alert(&#39;x&#39;)&lt;/script&gt;"));
        assert!(html.contains("&lt;img src=x onerror=alert(1)&gt;"));
        assert!(!html.contains("<script"));
        assert!(!html.contains("<img"));
    }

    #[test]
    fn countdown_percent_clamps_to_zero_for_expired_entries() {
        assert_eq!(countdown_percent(50, 200), 25);
        assert_eq!(countdown_percent(200, 200), 100);
        assert_eq!(countdown_percent(0, 200), 0);
        assert_eq!(countdown_percent(-1, 200), 0);
        assert_eq!(countdown_percent(10, 0), 0);
    }

    #[test]
    fn formats_epoch_seconds_as_iso8601_utc() {
        assert_eq!(format_epoch_seconds(0), "1970-01-01T00:00:00Z");
        assert_eq!(format_epoch_seconds(1_700_000_000), "2023-11-14T22:13:20Z");
    }
}
