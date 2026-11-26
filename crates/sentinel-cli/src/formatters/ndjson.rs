//! Newline-delimited JSON (NDJSON) formatter for contract scan reports.
//!
//! Emits one compact, self-contained JSON object per `StorageEntryReport` line,
//! enriched with top-level report context (`contract_id`, `network`,
//! `latest_ledger`, and `ledger_velocity_seconds`).

use sentinel_core::error::{SentinelError, SentinelResult};
use sentinel_core::usecases::TTLReport;
use serde::{Deserialize, Serialize};

use crate::formatters::json::JsonTTLReport;

/// A single NDJSON record combining report-level context with one storage entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NdjsonEntry {
    /// Target contract Strkey address.
    pub contract_id: String,
    /// Target network descriptor (e.g. "testnet", "mainnet").
    pub network: String,
    /// Latest ledger sequence number at time of inspection.
    pub latest_ledger: u32,
    /// Estimated ledger velocity in seconds per ledger (e.g. 5.0).
    pub ledger_velocity_seconds: f64,
    /// Identifier of the entry (e.g., "instance", "code_wasm", "persistent:Admin").
    pub identifier: String,
    /// Classification type (e.g., "contract_instance", "contract_code", "contract_data").
    pub entry_type: String,
    /// Durability tier (e.g., "instance", "code", "persistent").
    pub durability: String,
    /// Serialized size in bytes.
    pub size_bytes: usize,
    /// Expiration ledger sequence boundary.
    pub live_until_ledger: u32,
    /// Remaining ledgers until expiration.
    pub remaining_ledgers: i64,
    /// Estimated wall-clock seconds until expiration.
    pub estimated_remaining_seconds: f64,
    /// Estimated wall-clock days until expiration.
    pub estimated_remaining_days: f64,
    /// Lowercase risk status (e.g., "ok", "warn", "critical", "archived").
    pub status: String,
}

/// Converts a `TTLReport` into one [`NdjsonEntry`] per evaluated storage entry.
pub fn build_ndjson_entries(report: &TTLReport) -> Vec<NdjsonEntry> {
    let json_report = JsonTTLReport::from(report);
    json_report
        .entries
        .into_iter()
        .map(|entry| NdjsonEntry {
            contract_id: json_report.contract_id.clone(),
            network: json_report.network.clone(),
            latest_ledger: json_report.latest_ledger,
            ledger_velocity_seconds: json_report.ledger_velocity_seconds,
            identifier: entry.identifier,
            entry_type: entry.entry_type,
            durability: entry.durability,
            size_bytes: entry.size_bytes,
            live_until_ledger: entry.live_until_ledger,
            remaining_ledgers: entry.remaining_ledgers,
            estimated_remaining_seconds: entry.estimated_remaining_seconds,
            estimated_remaining_days: entry.estimated_remaining_days,
            status: entry.status,
        })
        .collect()
}

/// Renders a `TTLReport` as newline-delimited JSON (one compact JSON object per entry).
///
/// Returns an empty string when `report.entries` is empty.
pub fn render_ndjson(report: &TTLReport) -> SentinelResult<String> {
    let entries = build_ndjson_entries(report);
    let mut lines = Vec::with_capacity(entries.len());

    for entry in &entries {
        let line = serde_json::to_string(entry).map_err(|e| {
            SentinelError::SerializationError(format!(
                "Failed to serialize storage entry to NDJSON: {}",
                e
            ))
        })?;
        lines.push(line);
    }

    Ok(lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sentinel_core::domain::{Durability, EntryType, RiskLevel};
    use sentinel_core::usecases::StorageEntryReport;

    #[test]
    fn test_render_ndjson_emits_one_valid_json_object_per_entry() {
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
            burn_rate: Default::default(),
            entries,
            overall_status: RiskLevel::Warn,
            archival_recommendations: Vec::new(),
            sac_asset_info: None,
            contract_code_hash: None,
            contract_bytecode_size: None,
            contract_upgrade: None,
        };

        let output = render_ndjson(&report).expect("Should serialize cleanly to NDJSON");
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines.len(), 3);

        let parsed: Vec<NdjsonEntry> = lines
            .iter()
            .map(|line| serde_json::from_str(line).expect("Every NDJSON line must be valid JSON"))
            .collect();

        for item in &parsed {
            assert_eq!(item.contract_id, contract_id);
            assert_eq!(item.network, "testnet");
            assert_eq!(item.latest_ledger, 5_420_100);
            assert_eq!(item.ledger_velocity_seconds, 5.0);
        }

        assert_eq!(parsed[0].identifier, "instance");
        assert_eq!(parsed[0].entry_type, "contract_instance");
        assert_eq!(parsed[0].durability, "instance");
        assert_eq!(parsed[0].size_bytes, 342);
        assert_eq!(parsed[0].live_until_ledger, 5_530_100);
        assert_eq!(parsed[0].remaining_ledgers, 110_000);
        assert_eq!(parsed[0].estimated_remaining_seconds, 550_000.0);
        assert_eq!(parsed[0].estimated_remaining_days, 6.36);
        assert_eq!(parsed[0].status, "warn");

        assert_eq!(parsed[1].identifier, "code_wasm");
        assert_eq!(parsed[1].entry_type, "contract_code");
        assert_eq!(parsed[1].durability, "code");
        assert_eq!(parsed[1].size_bytes, 18_450);

        assert_eq!(parsed[2].identifier, "persistent:Admin");
        assert_eq!(parsed[2].entry_type, "contract_data");
        assert_eq!(parsed[2].durability, "persistent");
        assert_eq!(parsed[2].status, "ok");
    }

    #[test]
    fn test_render_ndjson_empty_entries_returns_empty_string() {
        let report = TTLReport {
            contract_id: "CEMPTY".to_string(),
            network: "mainnet".to_string(),
            latest_ledger: 5_000_000,
            ledger_velocity_seconds: 5.0,
            burn_rate: Default::default(),
            entries: vec![],
            overall_status: RiskLevel::Ok,
            archival_recommendations: Vec::new(),
            sac_asset_info: None,
            contract_code_hash: None,
            contract_bytecode_size: None,
            contract_upgrade: None,
        };

        let output = render_ndjson(&report).expect("Empty report should serialize cleanly");
        assert!(output.is_empty());
        assert_eq!(output.lines().count(), 0);
    }
}
