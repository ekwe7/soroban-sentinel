//! Flat, machine-readable CSV export for scan reports.
//!
//! Unlike the text formatter, this module never emits ANSI escape codes and
//! writes all numeric fields as plain integers. A single UTC ISO 8601
//! timestamp is generated per report and repeated on every row.

use std::time::{SystemTime, UNIX_EPOCH};

use sentinel_core::{
    derive_contract_instance_key, derive_persistent_key_from_scval, derive_persistent_symbol_key,
    derive_persistent_xdr_key, parse_contract_id, parse_key_expr, EntryType, RiskLevel,
    SentinelError, SentinelResult, StorageEntryReport, TTLReport,
};

/// Exact column order required by the tabular export contract.
pub const CSV_HEADER: &str = "timestamp,network,contract_id,key_xdr,durability,byte_size,live_until_ledger,remaining_ledgers,status";

/// One row of the shared tabular export, in `CSV_HEADER` column order.
///
/// Both the CSV and Parquet formatters build these rows through
/// [`build_tabular_rows`], so the two exports always carry the same values.
/// `key_xdr` is `None` when no ledger key XDR can be reconstructed (it is
/// rendered as an empty CSV field and as a null Parquet value).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabularRow {
    /// UTC timestamp for the whole report, as Unix seconds.
    pub timestamp_seconds: i64,
    pub network: String,
    pub contract_id: String,
    pub key_xdr: Option<String>,
    pub durability: &'static str,
    pub byte_size: i64,
    pub live_until_ledger: i64,
    pub remaining_ledgers: i64,
    pub status: &'static str,
}

/// Builds the flat rows shared by the CSV and Parquet exporters.
pub fn build_tabular_rows(report: &TTLReport) -> SentinelResult<Vec<TabularRow>> {
    let timestamp_seconds = unix_timestamp_now();
    let contract_id = parse_contract_id(&report.contract_id)?;

    report
        .entries
        .iter()
        .map(|entry| {
            let key_xdr = key_xdr_for_entry(&contract_id, entry)
                .unwrap_or_else(|_| entry.key_identifier.clone());

            Ok(TabularRow {
                timestamp_seconds,
                network: report.network.clone(),
                contract_id: report.contract_id.clone(),
                key_xdr: if key_xdr.is_empty() {
                    None
                } else {
                    Some(key_xdr)
                },
                durability: durability_label(entry),
                byte_size: i64::try_from(entry.byte_size).map_err(|_| {
                    SentinelError::SerializationError(format!(
                        "Entry byte size {} exceeds the tabular export limit",
                        entry.byte_size
                    ))
                })?,
                live_until_ledger: i64::from(entry.live_until_ledger),
                remaining_ledgers: entry.remaining_ledgers,
                status: status_label(entry),
            })
        })
        .collect()
}

/// Renders a scan report as CSV text, including the header row.
///
/// The returned string is terminated by a newline. Callers that print to a
/// terminal should trim that final newline before adding their own.
pub fn render_csv(report: &TTLReport) -> SentinelResult<String> {
    let rows = build_tabular_rows(report)?;
    let timestamp = format_epoch_seconds(
        rows.first()
            .map(|row| row.timestamp_seconds)
            .unwrap_or_else(unix_timestamp_now),
    );

    let mut writer = csv::WriterBuilder::new()
        .has_headers(false)
        .from_writer(Vec::new());

    writer
        .write_record(CSV_HEADER.split(','))
        .map_err(csv_error)?;

    for row in &rows {
        writer
            .write_record([
                timestamp.as_str(),
                row.network.as_str(),
                row.contract_id.as_str(),
                row.key_xdr.as_deref().unwrap_or(""),
                row.durability,
                row.byte_size.to_string().as_str(),
                row.live_until_ledger.to_string().as_str(),
                row.remaining_ledgers.to_string().as_str(),
                row.status,
            ])
            .map_err(csv_error)?;
    }

    writer.flush().map_err(|e| {
        SentinelError::SerializationError(format!("Failed to flush CSV output: {}", e))
    })?;
    let bytes = writer.into_inner().map_err(|e| {
        SentinelError::SerializationError(format!("Failed to finish CSV output: {}", e))
    })?;
    String::from_utf8(bytes)
        .map_err(|e| SentinelError::SerializationError(format!("CSV output was not UTF-8: {}", e)))
}

fn csv_error(err: csv::Error) -> SentinelError {
    SentinelError::SerializationError(format!("Failed to serialize CSV: {}", err))
}

fn durability_label(entry: &StorageEntryReport) -> &'static str {
    match entry.durability {
        sentinel_core::Durability::Instance => "instance",
        sentinel_core::Durability::Persistent => "persistent",
        sentinel_core::Durability::Temporary => "temporary",
        sentinel_core::Durability::Code => "code",
    }
}

fn status_label(entry: &StorageEntryReport) -> &'static str {
    match entry.status {
        RiskLevel::Ok => "ok",
        RiskLevel::Warn => "warn",
        RiskLevel::Critical => "critical",
        RiskLevel::Archived => "archived",
    }
}

/// Returns the base64-encoded ledger key XDR where it can be reconstructed
/// from the report. Contract code keys are left blank because the WASM hash
/// is not retained in `TTLReport`.
fn key_xdr_for_entry(contract_id: &[u8; 32], entry: &StorageEntryReport) -> SentinelResult<String> {
    match entry.entry_type {
        EntryType::ContractInstance => derive_contract_instance_key(contract_id),
        EntryType::ContractCode => Ok(String::new()),
        EntryType::PersistentData => {
            let identifier = entry.key_identifier.as_str();
            if let Some(raw) = identifier.strip_prefix("persistent_xdr:") {
                derive_persistent_xdr_key(contract_id, raw)
            } else if let Some(raw) = identifier.strip_prefix("persistent:") {
                match derive_persistent_symbol_key(contract_id, raw) {
                    Ok(key) => Ok(key),
                    Err(_) => {
                        let scval = parse_key_expr(raw)?;
                        derive_persistent_key_from_scval(contract_id, scval)
                    }
                }
            } else {
                Ok(identifier.to_string())
            }
        }
    }
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

    #[test]
    fn test_format_epoch_seconds() {
        assert_eq!(format_epoch_seconds(0), "1970-01-01T00:00:00Z");
        assert_eq!(format_epoch_seconds(1_000_000_000), "2001-09-09T01:46:40Z");
        assert_eq!(format_epoch_seconds(1_700_000_000), "2023-11-14T22:13:20Z");
    }

    #[test]
    fn test_csv_header_is_exact() {
        assert_eq!(
            CSV_HEADER,
            "timestamp,network,contract_id,key_xdr,durability,byte_size,live_until_ledger,remaining_ledgers,status"
        );
    }
}
