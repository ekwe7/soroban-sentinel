//! Storage placement audit models and cost-saving recommendations.

use serde::{Deserialize, Serialize};

pub const MAX_SOROBAN_ENTRY_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditFindingKind {
    SmallPersistentEntry,
    RapidExpiry,
    NearEntryLimit,
    LargePersistentEntry,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditFinding {
    pub kind: AuditFindingKind,
    pub severity: String,
    pub recommendation: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditEntry {
    pub identifier: String,
    pub durability: String,
    pub size_bytes: usize,
    pub live_until_ledger: u32,
    pub remaining_ledgers: i64,
    pub findings: Vec<AuditFinding>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StorageAuditReport {
    pub contract_id: String,
    pub network: String,
    pub latest_ledger: u32,
    pub total_entries: usize,
    pub persistent_entries: usize,
    pub temporary_entries: usize,
    pub instance_entries: usize,
    pub code_entries: usize,
    pub near_limit_entries: usize,
    pub estimated_annual_rent_savings_stroops: u64,
    pub entries: Vec<AuditEntry>,
    pub recommendations: Vec<String>,
}

pub fn analyze_storage_entry(
    identifier: impl Into<String>,
    durability: &str,
    size_bytes: usize,
    live_until_ledger: u32,
    remaining_ledgers: i64,
) -> AuditEntry {
    let mut findings = Vec::new();
    let identifier = identifier.into();

    if durability.eq_ignore_ascii_case("persistent") {
        if size_bytes < 32 {
            findings.push(AuditFinding {
                kind: AuditFindingKind::SmallPersistentEntry,
                severity: "recommendation".to_string(),
                recommendation: "This entry is smaller than 32 bytes; use temporary storage if it is recreated or does not need archival restoration.".to_string(),
            });
        }
        if (0..34_560).contains(&remaining_ledgers) {
            findings.push(AuditFinding {
                kind: AuditFindingKind::RapidExpiry,
                severity: "warning".to_string(),
                recommendation: "Persistent entry expires rapidly; extend its TTL or keep it temporary when archival restoration is unnecessary.".to_string(),
            });
        }
        if size_bytes >= (MAX_SOROBAN_ENTRY_BYTES * 80) / 100 {
            findings.push(AuditFinding {
                kind: AuditFindingKind::NearEntryLimit,
                severity: "warning".to_string(),
                recommendation: "Entry is within 20% of Soroban's 64 KiB entry limit; split collections or reconsider key/value boundaries.".to_string(),
            });
        }
        if size_bytes > MAX_SOROBAN_ENTRY_BYTES {
            findings.push(AuditFinding {
                kind: AuditFindingKind::LargePersistentEntry,
                severity: "critical".to_string(),
                recommendation: "Entry exceeds the 64 KiB Soroban network entry limit and must be split into smaller entries.".to_string(),
            });
        }
    }

    AuditEntry {
        identifier,
        durability: durability.to_string(),
        size_bytes,
        live_until_ledger,
        remaining_ledgers,
        findings,
    }
}

/// Transparent estimate: persistent placement premium assumes a 2x rent ratio.
pub fn estimate_annual_rent_savings(persistent_bytes: usize) -> u64 {
    const ANNUAL_LEDGER_EQUIVALENT: u64 = 6_307_200;
    const BASE_STROOPS_PER_BYTE_PER_LEDGER: u64 = 1;
    (persistent_bytes as u64)
        .saturating_mul(BASE_STROOPS_PER_BYTE_PER_LEDGER)
        .saturating_mul(ANNUAL_LEDGER_EQUIVALENT)
}
