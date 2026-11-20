//! Machine-readable JSON output formatter for contract scan reports.

use sentinel_core::domain::compare::BaselineDiff;
use sentinel_core::error::{SentinelError, SentinelResult};
use sentinel_core::usecases::TTLReport;
use serde::{Deserialize, Serialize};

/// Strict, schema-conforming JSON representation of a contract TTL report.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonTTLReport {
    /// Target contract Strkey address.
    pub contract_id: String,
    /// Target network descriptor (e.g. "testnet", "mainnet").
    pub network: String,
    /// Latest ledger sequence number at time of inspection.
    pub latest_ledger: u32,
    /// Estimated ledger velocity in seconds per ledger (e.g. 5.0).
    pub ledger_velocity_seconds: f64,
    /// Summary breakdown of entries by risk classification.
    pub summary: JsonSummary,
    /// Detailed list of evaluated storage entries.
    pub entries: Vec<JsonEntryReport>,
    /// Optional comparison against a baseline report.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub changes: Option<BaselineDiff>,
}

/// Numerical summary counts conforming to published JSON schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct JsonSummary {
    pub total_entries: usize,
    pub ok: usize,
    pub warn: usize,
    pub critical: usize,
    pub archived: usize,
}

/// Individual storage entry report conforming to published JSON schema.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonEntryReport {
    /// Identifier of the entry (e.g., "instance", "code_wasm", "persistent:Admin", "temporary:NONCE").
    pub identifier: String,
    /// Classification type (e.g., "contract_instance", "contract_code", "contract_data").
    pub entry_type: String,
    /// Durability tier (e.g., "instance", "code", "persistent", "temporary").
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
    /// `true` when a temporary entry is missing or expired: it is deleted permanently and
    /// cannot be restored. Omitted otherwise.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unrecoverable: bool,
    /// Instance-storage size metrics, present only on a found contract instance entry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance_storage: Option<JsonInstanceStorage>,
}

/// Instance-storage size metrics of a contract instance entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonInstanceStorage {
    /// Number of keys in the instance storage map.
    pub key_count: u32,
    /// Serialized size of the instance ledger entry in bytes.
    pub entry_size_bytes: usize,
    /// Network `max_contract_data_entry_bytes`; omitted if it could not be fetched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_entry_size_bytes: Option<u32>,
    /// Entry size as a percentage of the limit (2 decimals); omitted without the limit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub utilization_pct: Option<f64>,
    /// Lowercase status from entry size alone (e.g. "ok", "warn", "critical").
    pub size_status: String,
}

impl From<&TTLReport> for JsonTTLReport {
    fn from(report: &TTLReport) -> Self {
        let summary = report.summary();
        let json_summary = JsonSummary {
            total_entries: summary.total_entries,
            ok: summary.ok,
            warn: summary.warn,
            critical: summary.critical,
            archived: summary.archived,
        };

        let json_entries = report
            .entries
            .iter()
            .map(|entry| {
                let entry_type_str = match entry.entry_type {
                    sentinel_core::domain::EntryType::ContractInstance => "contract_instance",
                    sentinel_core::domain::EntryType::ContractCode => "contract_code",
                    sentinel_core::domain::EntryType::PersistentData
                    | sentinel_core::domain::EntryType::TemporaryData => "contract_data",
                };

                JsonEntryReport {
                    identifier: entry.key_identifier.clone(),
                    entry_type: entry_type_str.to_string(),
                    durability: entry.durability.to_string().to_lowercase(),
                    size_bytes: entry.byte_size,
                    live_until_ledger: entry.live_until_ledger,
                    remaining_ledgers: entry.remaining_ledgers,
                    estimated_remaining_seconds: entry.estimated_remaining_seconds,
                    estimated_remaining_days: (entry.remaining_days * 100.0).round() / 100.0,
                    status: entry.status.to_string().to_lowercase(),
                    unrecoverable: entry.is_unrecoverable(),
                    instance_storage: entry.instance_storage.as_ref().map(|metrics| {
                        JsonInstanceStorage {
                            key_count: metrics.key_count,
                            entry_size_bytes: metrics.entry_size_bytes,
                            max_entry_size_bytes: metrics.max_entry_size_bytes,
                            utilization_pct: metrics
                                .utilization_pct
                                .map(|pct| (pct * 100.0).round() / 100.0),
                            size_status: metrics.size_status.to_string().to_lowercase(),
                        }
                    }),
                }
            })
            .collect();

        Self {
            contract_id: report.contract_id.clone(),
            network: report.network.clone(),
            latest_ledger: report.latest_ledger,
            ledger_velocity_seconds: report.ledger_velocity_seconds,
            summary: json_summary,
            entries: json_entries,
            changes: None,
        }
    }
}

/// Renders a `TTLReport` into strict, pretty-printed JSON conforming to published schema.
pub fn render_json(report: &TTLReport) -> SentinelResult<String> {
    render_json_with_diff(report, None)
}

/// Renders a `TTLReport` and optional `BaselineDiff` into pretty-printed JSON.
pub fn render_json_with_diff(
    report: &TTLReport,
    changes: Option<BaselineDiff>,
) -> SentinelResult<String> {
    let mut json_report = JsonTTLReport::from(report);
    json_report.changes = changes;
    serde_json::to_string_pretty(&json_report).map_err(|e| {
        SentinelError::SerializationError(format!("Failed to serialize report to JSON: {}", e))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use sentinel_core::domain::{Durability, EntryType, RiskLevel};
    use sentinel_core::usecases::StorageEntryReport;

    #[test]
    fn test_render_json_matches_mvp_schema() {
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
            entries,
            overall_status: RiskLevel::Warn,
            archival_recommendations: Vec::new(),
        };

        let json_str = render_json(&report).expect("Should serialize cleanly to JSON");

        // Parse as generic JSON Value to validate exact schema shape and keys
        let parsed: serde_json::Value =
            serde_json::from_str(&json_str).expect("Output must be valid JSON");

        assert_eq!(parsed["contract_id"], contract_id);
        assert_eq!(parsed["network"], "testnet");
        assert_eq!(parsed["latest_ledger"], 5_420_100);
        assert_eq!(parsed["ledger_velocity_seconds"], 5.0);

        // Validate summary fields
        assert_eq!(parsed["summary"]["total_entries"], 3);
        assert_eq!(parsed["summary"]["ok"], 1);
        assert_eq!(parsed["summary"]["warn"], 2);
        assert_eq!(parsed["summary"]["critical"], 0);
        assert_eq!(parsed["summary"]["archived"], 0);

        // Validate entries
        let entries_arr = parsed["entries"].as_array().expect("entries must be array");
        assert_eq!(entries_arr.len(), 3);

        let inst = &entries_arr[0];
        assert_eq!(inst["identifier"], "instance");
        assert_eq!(inst["entry_type"], "contract_instance");
        assert_eq!(inst["durability"], "instance");
        assert_eq!(inst["size_bytes"], 342);
        assert_eq!(inst["live_until_ledger"], 5_530_100);
        assert_eq!(inst["remaining_ledgers"], 110_000);
        assert_eq!(inst["estimated_remaining_seconds"], 550_000.0);
        assert_eq!(inst["estimated_remaining_days"], 6.36);
        assert_eq!(inst["status"], "warn");

        let code = &entries_arr[1];
        assert_eq!(code["identifier"], "code_wasm");
        assert_eq!(code["entry_type"], "contract_code");
        assert_eq!(code["durability"], "code");
        assert_eq!(code["size_bytes"], 18_450);
        assert_eq!(code["status"], "warn");

        let admin = &entries_arr[2];
        assert_eq!(admin["identifier"], "persistent:Admin");
        assert_eq!(admin["entry_type"], "contract_data");
        assert_eq!(admin["durability"], "persistent");
        assert_eq!(admin["size_bytes"], 64);
        assert_eq!(admin["status"], "ok");
    }

    #[test]
    fn test_render_json_roundtrip_deserialization() {
        let entries = vec![StorageEntryReport::new(
            "instance",
            EntryType::ContractInstance,
            Durability::Instance,
            100,
            5_500_000,
            80_000,
            400_000.0,
            4.63,
            RiskLevel::Ok,
        )];

        let report = TTLReport {
            contract_id: "CCONTRACT456".to_string(),
            network: "mainnet".to_string(),
            latest_ledger: 5_420_000,
            ledger_velocity_seconds: 5.0,
            entries,
            overall_status: RiskLevel::Ok,
            archival_recommendations: Vec::new(),
        };

        let json_str = render_json(&report).unwrap();
        let deserialized: JsonTTLReport =
            serde_json::from_str(&json_str).expect("Should deserialize back into JsonTTLReport");

        assert_eq!(deserialized.contract_id, "CCONTRACT456");
        assert_eq!(deserialized.network, "mainnet");
        assert_eq!(deserialized.latest_ledger, 5_420_000);
        assert_eq!(deserialized.entries.len(), 1);
        assert_eq!(deserialized.entries[0].identifier, "instance");
        assert_eq!(deserialized.entries[0].status, "ok");
    }

    #[test]
    fn test_render_json_empty_entries() {
        let report = TTLReport {
            contract_id: "CEMPTY".to_string(),
            network: "testnet".to_string(),
            latest_ledger: 5_000_000,
            ledger_velocity_seconds: 5.0,
            entries: vec![],
            overall_status: RiskLevel::Ok,
            archival_recommendations: Vec::new(),
        };

        let json_str = render_json(&report).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();

        assert_eq!(parsed["summary"]["total_entries"], 0);
        assert_eq!(parsed["entries"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn test_render_json_temporary_entries() {
        let report = TTLReport {
            contract_id: "CTEMP".to_string(),
            network: "testnet".to_string(),
            latest_ledger: 5_400_000,
            ledger_velocity_seconds: 5.0,
            entries: vec![
                StorageEntryReport::new(
                    "temporary:NONCE",
                    EntryType::TemporaryData,
                    Durability::Temporary,
                    48,
                    5_450_000,
                    50_000,
                    250_000.0,
                    2.89,
                    RiskLevel::Warn,
                ),
                StorageEntryReport::new(
                    "temporary:SESSION",
                    EntryType::TemporaryData,
                    Durability::Temporary,
                    0,
                    0,
                    0,
                    0.0,
                    0.0,
                    RiskLevel::Archived,
                ),
            ],
            overall_status: RiskLevel::Archived,
            archival_recommendations: Vec::new(),
        };

        let parsed: serde_json::Value =
            serde_json::from_str(&render_json(&report).unwrap()).unwrap();
        let live = &parsed["entries"][0];
        assert_eq!(live["identifier"], "temporary:NONCE");
        assert_eq!(live["durability"], "temporary");
        assert_eq!(live["entry_type"], "contract_data");
        assert!(live.get("unrecoverable").is_none());

        let gone = &parsed["entries"][1];
        assert_eq!(gone["identifier"], "temporary:SESSION");
        assert_eq!(gone["status"], "archived");
        assert_eq!(gone["unrecoverable"], true);
    }

    #[test]
    fn test_render_json_instance_storage_metrics() {
        let mut with_limit = StorageEntryReport::new(
            "instance",
            EntryType::ContractInstance,
            Durability::Instance,
            52_000,
            5_900_000,
            500_000,
            2_500_000.0,
            28.9,
            RiskLevel::Warn,
        );
        with_limit.instance_storage = Some(sentinel_core::usecases::InstanceStorageReport {
            key_count: 60,
            entry_size_bytes: 52_429,
            max_entry_size_bytes: Some(65_536),
            utilization_pct: Some(80.000_15),
            size_status: RiskLevel::Warn,
        });
        let mut without_limit = with_limit.clone();
        without_limit.instance_storage = Some(sentinel_core::usecases::InstanceStorageReport {
            key_count: 60,
            entry_size_bytes: 52_429,
            max_entry_size_bytes: None,
            utilization_pct: None,
            size_status: RiskLevel::Ok,
        });
        let code = StorageEntryReport::new(
            "code_wasm",
            EntryType::ContractCode,
            Durability::Code,
            1,
            5_900_000,
            500_000,
            1.0,
            1.0,
            RiskLevel::Ok,
        );

        for (entry, limit_known) in [(with_limit, true), (without_limit, false)] {
            let report = TTLReport {
                contract_id: "CSIZE".to_string(),
                network: "testnet".to_string(),
                latest_ledger: 5_400_000,
                ledger_velocity_seconds: 5.0,
                entries: vec![entry, code.clone()],
                overall_status: RiskLevel::Warn,
                archival_recommendations: Vec::new(),
            };
            let parsed: serde_json::Value =
                serde_json::from_str(&render_json(&report).unwrap()).unwrap();
            let metrics = &parsed["entries"][0]["instance_storage"];
            assert_eq!(metrics["key_count"], 60);
            assert_eq!(metrics["entry_size_bytes"], 52_429);
            if limit_known {
                assert_eq!(metrics["max_entry_size_bytes"], 65_536);
                assert_eq!(metrics["utilization_pct"], 80.0);
                assert_eq!(metrics["size_status"], "warn");
            } else {
                assert!(metrics.get("max_entry_size_bytes").is_none());
                assert!(metrics.get("utilization_pct").is_none());
                assert_eq!(metrics["size_status"], "ok");
            }
            assert!(parsed["entries"][1].get("instance_storage").is_none());
        }
    }
}
