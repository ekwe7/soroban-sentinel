//! Integration tests for the JUnit XML output formatter (Issue 122: --format junit).
//!
//! These tests validate the JUnit XML renderer for `sentinel scan` across all
//! risk levels, XML escaping, and formatter wiring.

use sentinel_core::domain::{Durability, EntryType, RiskLevel};
use sentinel_core::usecases::StorageEntryReport;

fn make_entry(
    identifier: &str,
    live_until: u32,
    remaining: i64,
    status: RiskLevel,
) -> StorageEntryReport {
    StorageEntryReport::new(
        identifier,
        EntryType::PersistentData,
        Durability::Persistent,
        64,
        live_until,
        remaining,
        remaining as f64 * 5.0,
        remaining as f64 * 5.0 / 86400.0,
        status,
    )
}

fn make_report(entries: Vec<StorageEntryReport>) -> sentinel_core::usecases::TTLReport {
    sentinel_core::usecases::TTLReport {
        contract_id: "CAAA".to_string(),
        network: "testnet".to_string(),
        latest_ledger: 5_400_000,
        ledger_velocity_seconds: 5.0,
        entries,
        overall_status: sentinel_core::domain::RiskLevel::Ok,
        archival_recommendations: Vec::new(),
        sac_asset_info: None,
        burn_rate: sentinel_core::domain::BurnRate::fallback(),
        contract_code_hash: None,
        contract_bytecode_size: None,
        contract_upgrade: None,
    }
}

/// Renders the JUnit XML output for a report and returns it as a string.
fn junit_xml_report(entries: &[StorageEntryReport]) -> String {
    let report = make_report(entries.to_vec());
    sentinel_cli::formatters::junit::render_junit(&report).unwrap()
}

#[test]
fn junit_output_valid_xml_for_ok_entries() {
    let entries = vec![make_entry(
        "persistent:COUNTER",
        5_800_000,
        400_000,
        RiskLevel::Ok,
    )];
    let xml = junit_xml_report(&entries);
    println!("=== JUnit output for Ok ===\n{}", xml);
    assert!(xml.contains("<testsuite"));
    assert!(xml.contains("<testcase"));
    assert!(xml.contains("</testsuites>"));
    // Ok entries produce no failure element
}

#[test]
fn junit_output_valid_xml_for_warn_entries() {
    let entries = vec![make_entry(
        "persistent:COUNTER",
        5_500_000,
        100_000,
        RiskLevel::Warn,
    )];
    let xml = junit_xml_report(&entries);
    println!("=== JUnit output for Warn ===\n{}", xml);
    assert!(xml.contains("<skipped"));
}

#[test]
fn junit_output_valid_xml_for_critical_entries() {
    let entries = vec![make_entry(
        "persistent:COUNTER",
        5_410_000,
        10_000,
        RiskLevel::Critical,
    )];
    let xml = junit_xml_report(&entries);
    println!("=== JUnit output for Critical ===\n{}", xml);
    assert!(xml.contains("<failure"));
    assert!(xml.contains("risk=critical"));
}

#[test]
fn junit_output_valid_xml_for_archived_entries() {
    let entries = vec![make_entry("persistent:COUNTER", 0, 0, RiskLevel::Archived)];
    let xml = junit_xml_report(&entries);
    println!("=== JUnit output for Archived ===\n{}", xml);
    assert!(xml.contains("<failure"));
    assert!(xml.contains("risk=archived"));
}

#[test]
fn junit_output_xml_escapes_identifiers() {
    let entries = vec![make_entry(
        "persistent:ADMIN&TOKEN",
        5_800_000,
        400_000,
        RiskLevel::Ok,
    )];
    let xml = junit_xml_report(&entries);
    assert!(xml.contains("ADMIN&amp;TOKEN"));
    assert!(!xml.contains("ADMIN&TOKEN"));
}

#[test]
fn junit_output_xml_escapes_network_and_key_identifiers() {
    let entries = vec![make_entry(
        "persistent:ESCAPE<>&",
        5_800_000,
        400_000,
        RiskLevel::Ok,
    )];
    let xml = junit_xml_report(&entries);
    println!("=== JUnit output for ESCAPE ===\n{}", xml);
    assert!(xml.contains("ESCAPE&lt;&gt;&amp;"));
}
