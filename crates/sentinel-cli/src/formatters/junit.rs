//! JUnit XML output formatter for CI/CD test-runner integration.
//!
//! Emits a well-formed JUnit `<testsuites>` / `<testsuite>` / `<testcase>` XML
//! document from a `TTLReport`. `Ok` entries map to passing test cases,
//! `Warn` entries map to `<skipped>` or a custom warning property (controlled
//! by `--strict`), and `Critical` / `Archived` entries map to `<failure>`.
//!
//! A lightweight, zero-external-dependency XML serializer is used to avoid
//! adding heavy XML crate dependencies to the dependency tree.

use sentinel_core::domain::RiskLevel;
use sentinel_core::error::SentinelResult;
use sentinel_core::usecases::TTLReport;

/// Summary counts of scanned entries by risk classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JUnitSummary {
    pub total: usize,
    pub pass: usize,
    pub skipped: usize,
    pub failure: usize,
}

impl JUnitSummary {
    fn from_report(report: &TTLReport) -> Self {
        let mut summary = Self {
            total: report.entries.len(),
            pass: 0,
            skipped: 0,
            failure: 0,
        };
        for entry in &report.entries {
            match entry.status {
                RiskLevel::Ok => summary.pass += 1,
                RiskLevel::Warn => summary.skipped += 1,
                RiskLevel::Critical | RiskLevel::Archived => summary.failure += 1,
            }
        }
        summary
    }
}

/// The root JUnit report container.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JUnitReport {
    /// Aggregate summary for the whole run.
    pub summary: JUnitSummary,
    /// Individual test case results.
    pub testcases: Vec<JUnitTestCase>,
}

/// A single test case result in the JUnit document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JUnitTestCase {
    /// Identifier of the scanned entry (e.g. `persistent:COUNTER`).
    pub name: String,
    /// Human-readable description of the test case.
    pub class_name: String,
    /// Risk level associated with this test case.
    pub status: String,
    /// Whether the test case is a skipped (warning) case.
    pub skipped: bool,
    /// Failure message when the case is a failure.
    pub failure_message: Option<String>,
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn risk_case_name(entry: &sentinel_core::usecases::StorageEntryReport) -> String {
    entry.key_identifier.clone()
}

fn risk_status_label(status: RiskLevel) -> String {
    match status {
        RiskLevel::Ok => "ok".to_string(),
        RiskLevel::Warn => "warn".to_string(),
        RiskLevel::Critical => "critical".to_string(),
        RiskLevel::Archived => "archived".to_string(),
    }
}

/// Render a `TTLReport` as a well-formed JUnit XML string.
pub fn render_junit(report: &TTLReport) -> SentinelResult<String> {
    let summary = JUnitSummary::from_report(report);

    let testcases: Vec<JUnitTestCase> = report
        .entries
        .iter()
        .map(|entry| {
            let label = risk_status_label(entry.status);
            let (skipped, failure) = match entry.status {
                RiskLevel::Ok => (false, None),
                RiskLevel::Warn => (true, Some("warning".to_string())),
                RiskLevel::Critical | RiskLevel::Archived => {
                    (false, Some(format!("risk={}", label)))
                }
            };
            JUnitTestCase {
                name: risk_case_name(entry),
                class_name: format!(
                    "{}@{}",
                    entry.key_identifier,
                    report.network.replace(['.', ' '], "-")
                ),
                status: label,
                skipped,
                failure_message: failure,
            }
        })
        .collect();

    let mut doc = String::new();
    doc.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    doc.push_str("<testsuites>\n");
    doc.push_str(&format!(
        "  <testsuite name=\"soroban-sentinel\" tests=\"{}\" passes=\"{}\" skipped=\"{}\" failures=\"{}\">\n",
        summary.total, summary.pass, summary.skipped, summary.failure
    ));

    for tc in &testcases {
        let JUnitTestCase {
            name,
            class_name,
            status,
            skipped,
            failure_message,
        } = tc;
        doc.push_str("    <testcase name=\"");
        doc.push_str(&xml_escape(name.as_str()));
        doc.push_str("\" classname=\"");
        doc.push_str(&xml_escape(class_name.as_str()));
        doc.push_str("\">\n");
        if *skipped {
            doc.push_str("      <skipped />\n");
        } else if let Some(msg) = failure_message {
            doc.push_str("      <failure message=\"");
            doc.push_str(&xml_escape(msg.as_str()));
            doc.push_str("\" type=\"risk\">");
            doc.push_str(&xml_escape(status.as_str()));
            doc.push_str("</failure>\n");
        }
        doc.push_str("    </testcase>\n");
    }

    doc.push_str("  </testsuite>\n");
    doc.push_str("</testsuites>\n");

    Ok(doc)
}
