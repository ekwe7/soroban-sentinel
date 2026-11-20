//! SARIF v2.1.0 output formatter for GitHub Code Scanning.

use sentinel_core::domain::RiskLevel;
use sentinel_core::error::{SentinelError, SentinelResult};
use sentinel_core::usecases::TTLReport;
use serde::Serialize;

const SARIF_SCHEMA: &str = "https://json.schemastore.org/sarif-2.1.0.json";

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SarifLog {
    #[serde(rename = "$schema")]
    schema: &'static str,
    version: &'static str,
    runs: Vec<SarifRun>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SarifRun {
    tool: SarifTool,
    results: Vec<SarifResult>,
}

#[derive(Debug, Serialize)]
struct SarifTool {
    driver: SarifDriver,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SarifDriver {
    name: &'static str,
    information_uri: &'static str,
    rules: Vec<SarifRule>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SarifRule {
    id: &'static str,
    short_description: SarifMessage,
    help_uri: &'static str,
}

#[derive(Debug, Serialize)]
struct SarifMessage {
    text: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SarifResult {
    rule_id: &'static str,
    level: &'static str,
    message: SarifMessage,
    locations: Vec<SarifLocation>,
}

#[derive(Debug, Serialize)]
struct SarifLocation {
    #[serde(rename = "physicalLocation")]
    physical_location: SarifPhysicalLocation,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SarifPhysicalLocation {
    artifact_location: SarifArtifactLocation,
}

#[derive(Debug, Serialize)]
struct SarifArtifactLocation {
    uri: String,
}

fn risk_rule(status: RiskLevel) -> Option<(&'static str, &'static str)> {
    match status {
        RiskLevel::Warn => Some(("SENTINEL-TTL-WARN", "warning")),
        RiskLevel::Critical => Some(("SENTINEL-TTL-CRITICAL", "error")),
        RiskLevel::Archived => Some(("SENTINEL-TTL-ARCHIVED", "error")),
        RiskLevel::Ok => None,
    }
}

/// Renders a contract TTL report as SARIF v2.1.0 JSON.
pub fn render_sarif(report: &TTLReport) -> SentinelResult<String> {
    let rules = vec![
        SarifRule {
            id: "SENTINEL-TTL-WARN",
            short_description: SarifMessage {
                text: "Storage TTL warning".to_string(),
            },
            help_uri: "https://github.com/ekwe7/soroban-sentinel#ttl-risk-statuses",
        },
        SarifRule {
            id: "SENTINEL-TTL-CRITICAL",
            short_description: SarifMessage {
                text: "Critical storage TTL risk".to_string(),
            },
            help_uri: "https://github.com/ekwe7/soroban-sentinel#ttl-risk-statuses",
        },
        SarifRule {
            id: "SENTINEL-TTL-ARCHIVED",
            short_description: SarifMessage {
                text: "Archived Soroban ledger entry".to_string(),
            },
            help_uri: "https://github.com/ekwe7/soroban-sentinel#ttl-risk-statuses",
        },
    ];

    let results = report
        .entries
        .iter()
        .filter_map(|entry| {
            let (rule_id, level) = risk_rule(entry.status)?;
            let key = entry.key_identifier.trim_start_matches("persistent:");
            let message = format!(
                "Contract {} storage entry '{}' expires at ledger {} with {} ledgers remaining ({})",
                report.contract_id,
                key,
                entry.live_until_ledger,
                entry.remaining_ledgers,
                entry.status
            );

            Some(SarifResult {
                rule_id,
                level,
                message: SarifMessage { text: message },
                locations: vec![SarifLocation {
                    physical_location: SarifPhysicalLocation {
                        artifact_location: SarifArtifactLocation {
                            uri: format!("soroban://{}/{}", report.contract_id, key),
                        },
                    },
                }],
            })
        })
        .collect();

    let log = SarifLog {
        schema: SARIF_SCHEMA,
        version: "2.1.0",
        runs: vec![SarifRun {
            tool: SarifTool {
                driver: SarifDriver {
                    name: "soroban-sentinel",
                    information_uri: "https://github.com/ekwe7/soroban-sentinel",
                    rules,
                },
            },
            results,
        }],
    };

    serde_json::to_string_pretty(&log).map_err(|error| {
        SentinelError::SerializationError(format!("Failed to serialize report to SARIF: {error}"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use sentinel_core::domain::{Durability, EntryType};
    use sentinel_core::usecases::StorageEntryReport;

    #[test]
    fn sarif_maps_warn_and_error_levels() {
        let report = TTLReport {
            contract_id: "C123".to_string(),
            network: "testnet".to_string(),
            latest_ledger: 100,
            ledger_velocity_seconds: 5.0,
            entries: vec![
                StorageEntryReport::new(
                    "persistent:ADMIN",
                    EntryType::PersistentData,
                    Durability::Persistent,
                    64,
                    150,
                    50,
                    250.0,
                    0.01,
                    RiskLevel::Warn,
                ),
                StorageEntryReport::new(
                    "persistent:COUNTER",
                    EntryType::PersistentData,
                    Durability::Persistent,
                    64,
                    120,
                    20,
                    100.0,
                    0.01,
                    RiskLevel::Critical,
                ),
            ],
            overall_status: RiskLevel::Critical,
            archival_recommendations: Vec::new(),
        };

        let json = render_sarif(&report).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["version"], "2.1.0");
        assert_eq!(
            value["runs"][0]["tool"]["driver"]["name"],
            "soroban-sentinel"
        );
        assert_eq!(value["runs"][0]["results"][0]["level"], "warning");
        assert_eq!(value["runs"][0]["results"][1]["level"], "error");
    }
}
