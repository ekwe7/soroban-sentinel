//! SonarQube Generic Issue Import JSON formatter.

use sentinel_core::domain::RiskLevel;
use sentinel_core::error::{SentinelError, SentinelResult};
use sentinel_core::usecases::{StorageEntryReport, TTLReport};
use serde::Serialize;

const ENGINE_ID: &str = "soroban-sentinel";

#[derive(Debug, Serialize)]
struct SonarQubeReport {
    rules: Vec<SonarQubeRule>,
    issues: Vec<SonarQubeIssue>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SonarQubeRule {
    id: &'static str,
    name: &'static str,
    description: &'static str,
    engine_id: &'static str,
    clean_code_attribute: &'static str,
    #[serde(rename = "type")]
    rule_type: &'static str,
    severity: &'static str,
    impacts: Vec<SonarQubeImpact>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SonarQubeImpact {
    software_quality: &'static str,
    severity: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SonarQubeIssue {
    engine_id: &'static str,
    rule_id: &'static str,
    #[serde(rename = "type")]
    issue_type: &'static str,
    severity: &'static str,
    primary_location: SonarQubeLocation,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SonarQubeLocation {
    message: String,
    file_path: String,
}

#[derive(Clone, Copy)]
struct RuleSpec {
    id: &'static str,
    name: &'static str,
    description: &'static str,
    clean_code_attribute: &'static str,
    issue_type: &'static str,
    severity: &'static str,
    software_quality: &'static str,
    impact_severity: &'static str,
}

fn rule_spec(status: RiskLevel) -> Option<RuleSpec> {
    match status {
        RiskLevel::Ok => None,
        RiskLevel::Warn => Some(RuleSpec {
            id: "SENTINEL-TTL-WARN",
            name: "Soroban storage TTL warning",
            description: "A storage entry is below the configured TTL warning threshold.",
            clean_code_attribute: "COMPLETE",
            issue_type: "CODE_SMELL",
            severity: "MAJOR",
            software_quality: "MAINTAINABILITY",
            impact_severity: "MEDIUM",
        }),
        RiskLevel::Critical => Some(RuleSpec {
            id: "SENTINEL-TTL-CRITICAL",
            name: "Critical Soroban storage TTL",
            description: "A storage entry is below the configured TTL critical threshold.",
            clean_code_attribute: "TRUSTWORTHY",
            issue_type: "VULNERABILITY",
            severity: "CRITICAL",
            software_quality: "SECURITY",
            impact_severity: "HIGH",
        }),
        RiskLevel::Archived => Some(RuleSpec {
            id: "SENTINEL-TTL-ARCHIVED",
            name: "Archived Soroban storage entry",
            description: "A storage entry has expired and is no longer live on the ledger.",
            clean_code_attribute: "TRUSTWORTHY",
            issue_type: "VULNERABILITY",
            severity: "BLOCKER",
            software_quality: "SECURITY",
            impact_severity: "BLOCKER",
        }),
    }
}

fn rule_for(spec: RuleSpec) -> SonarQubeRule {
    SonarQubeRule {
        id: spec.id,
        name: spec.name,
        description: spec.description,
        engine_id: ENGINE_ID,
        clean_code_attribute: spec.clean_code_attribute,
        rule_type: spec.issue_type,
        severity: spec.severity,
        impacts: vec![SonarQubeImpact {
            software_quality: spec.software_quality,
            severity: spec.impact_severity,
        }],
    }
}

fn issue_for(report: &TTLReport, entry: &StorageEntryReport, spec: RuleSpec) -> SonarQubeIssue {
    let key = entry
        .key_identifier
        .strip_prefix("persistent:")
        .unwrap_or(&entry.key_identifier);
    let key_hex = key
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    SonarQubeIssue {
        engine_id: ENGINE_ID,
        rule_id: spec.id,
        issue_type: spec.issue_type,
        severity: spec.severity,
        primary_location: SonarQubeLocation {
            message: format!(
                "Contract {} storage entry '{}' is {} with {} ledgers remaining; expiry is at ledger {}.",
                report.contract_id,
                key,
                entry.status,
                entry.remaining_ledgers,
                entry.live_until_ledger,
            ),
            file_path: format!("soroban/{}/{key_hex}.ttl", report.contract_id),
        },
    }
}

/// Renders TTL risks as SonarQube Generic Issue Import JSON.
pub fn render_sonarqube(report: &TTLReport) -> SentinelResult<String> {
    let mut rules = Vec::new();
    let mut issues = Vec::new();

    for status in [RiskLevel::Warn, RiskLevel::Critical, RiskLevel::Archived] {
        let Some(spec) = rule_spec(status) else {
            continue;
        };
        let entries = report
            .entries
            .iter()
            .filter(|entry| entry.status == status)
            .collect::<Vec<_>>();
        if entries.is_empty() {
            continue;
        }
        rules.push(rule_for(spec));
        issues.extend(
            entries
                .into_iter()
                .map(|entry| issue_for(report, entry, spec)),
        );
    }

    serde_json::to_string_pretty(&SonarQubeReport { rules, issues }).map_err(|error| {
        SentinelError::SerializationError(format!(
            "Failed to serialize report to SonarQube Generic Issue JSON: {error}"
        ))
    })
}
