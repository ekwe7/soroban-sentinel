//! Pure domain comparison engine for diffing contract TTL scan reports against historical baselines.

use serde::{Deserialize, Serialize};

use crate::domain::RiskLevel;
use crate::usecases::TTLReport;

/// Maximum allowed baseline file byte size (5 MB) to prevent unbounded memory allocation.
pub const MAX_BASELINE_FILE_BYTES: u64 = 5 * 1024 * 1024;

/// Categorization of changes observed for an individual storage entry between baseline and current scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeCategory {
    /// Live-until ledger boundary was extended into the future.
    Extended,
    /// Live-until ledger, remaining horizon, and risk status are identical.
    Unchanged,
    /// Live-until ledger unchanged, but network time progressed so remaining ledgers decayed.
    Decayed,
    /// Entry risk status transitioned to a worse level (e.g. OK -> WARN, WARN -> CRITICAL).
    StatusWorsened,
    /// Entry risk status transitioned to a healthier level (e.g. CRITICAL -> WARN, WARN -> OK).
    StatusImproved,
    /// Entry was not present in baseline report but is present in current scan.
    Added,
    /// Entry was present in baseline report but is missing from current scan.
    Removed,
}

impl std::fmt::Display for ChangeCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Extended => write!(f, "extended"),
            Self::Unchanged => write!(f, "unchanged"),
            Self::Decayed => write!(f, "decayed"),
            Self::StatusWorsened => write!(f, "status_worsened"),
            Self::StatusImproved => write!(f, "status_improved"),
            Self::Added => write!(f, "added"),
            Self::Removed => write!(f, "removed"),
        }
    }
}

/// Numerical counts summarizing changes across all evaluated entries against a baseline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BaselineDiffSummary {
    pub extended: usize,
    pub unchanged: usize,
    pub decayed: usize,
    pub status_worsened: usize,
    pub status_improved: usize,
    pub added: usize,
    pub removed: usize,
    pub has_regression: bool,
}

/// Detailed change breakdown for an individual storage entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EntryDiff {
    pub identifier: String,
    pub category: ChangeCategory,
    pub baseline_live_until: u32,
    pub current_live_until: u32,
    pub live_until_delta: i64,
    pub baseline_status: Option<RiskLevel>,
    pub current_status: Option<RiskLevel>,
}

/// Complete typed diff between a current scan report and a previous baseline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BaselineDiff {
    pub baseline_latest_ledger: u32,
    pub current_latest_ledger: u32,
    pub ledgers_elapsed: i64,
    pub summary: BaselineDiffSummary,
    pub entries: Vec<EntryDiff>,
}

/// Snapshot structure used for deserializing arbitrary previous JSON scan reports.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BaselineSnapshot {
    pub contract_id: String,
    pub network: String,
    pub latest_ledger: u32,
    #[serde(default)]
    pub entries: Vec<BaselineEntrySnapshot>,
}

/// Entry report subset deserialized from previous baseline JSON.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BaselineEntrySnapshot {
    pub identifier: String,
    #[serde(default)]
    pub live_until_ledger: u32,
    #[serde(default)]
    pub remaining_ledgers: i64,
    pub status: String,
}

/// Compares a current `TTLReport` against a deserialized historical `BaselineSnapshot`.
pub fn compare_reports(current: &TTLReport, baseline: &BaselineSnapshot) -> BaselineDiff {
    let ledgers_elapsed = (current.latest_ledger as i64) - (baseline.latest_ledger as i64);

    let mut entry_diffs = Vec::new();
    let mut extended = 0;
    let mut unchanged = 0;
    let mut decayed = 0;
    let mut status_worsened = 0;
    let mut status_improved = 0;
    let mut added = 0;
    let mut removed = 0;

    // 1. Check current entries against baseline
    for current_entry in &current.entries {
        let baseline_match = baseline
            .entries
            .iter()
            .find(|b| b.identifier == current_entry.key_identifier);

        if let Some(b) = baseline_match {
            let b_status = parse_risk_level(&b.status);
            let b_live_until = b.live_until_ledger;
            let c_live_until = current_entry.live_until_ledger;
            let live_until_delta = (c_live_until as i64) - (b_live_until as i64);

            let category = if c_live_until > b_live_until {
                ChangeCategory::Extended
            } else {
                let current_rank = status_severity(current_entry.status);
                let baseline_rank = status_severity(b_status);

                if current_rank > baseline_rank {
                    ChangeCategory::StatusWorsened
                } else if current_rank < baseline_rank {
                    ChangeCategory::StatusImproved
                } else if ledgers_elapsed > 0
                    || current_entry.remaining_ledgers < b.remaining_ledgers
                {
                    ChangeCategory::Decayed
                } else {
                    ChangeCategory::Unchanged
                }
            };

            match category {
                ChangeCategory::Extended => extended += 1,
                ChangeCategory::Unchanged => unchanged += 1,
                ChangeCategory::Decayed => decayed += 1,
                ChangeCategory::StatusWorsened => status_worsened += 1,
                ChangeCategory::StatusImproved => status_improved += 1,
                ChangeCategory::Added => added += 1,
                ChangeCategory::Removed => removed += 1,
            }

            entry_diffs.push(EntryDiff {
                identifier: current_entry.key_identifier.clone(),
                category,
                baseline_live_until: b_live_until,
                current_live_until: c_live_until,
                live_until_delta,
                baseline_status: Some(b_status),
                current_status: Some(current_entry.status),
            });
        } else {
            // New entry not in baseline
            added += 1;
            entry_diffs.push(EntryDiff {
                identifier: current_entry.key_identifier.clone(),
                category: ChangeCategory::Added,
                baseline_live_until: 0,
                current_live_until: current_entry.live_until_ledger,
                live_until_delta: current_entry.live_until_ledger as i64,
                baseline_status: None,
                current_status: Some(current_entry.status),
            });
        }
    }

    // 2. Check for removed entries (in baseline but not in current)
    for b in &baseline.entries {
        let in_current = current
            .entries
            .iter()
            .any(|c| c.key_identifier == b.identifier);
        if !in_current {
            removed += 1;
            let b_status = parse_risk_level(&b.status);
            entry_diffs.push(EntryDiff {
                identifier: b.identifier.clone(),
                category: ChangeCategory::Removed,
                baseline_live_until: b.live_until_ledger,
                current_live_until: 0,
                live_until_delta: -(b.live_until_ledger as i64),
                baseline_status: Some(b_status),
                current_status: None,
            });
        }
    }

    let has_regression = status_worsened > 0 || removed > 0;

    let summary = BaselineDiffSummary {
        extended,
        unchanged,
        decayed,
        status_worsened,
        status_improved,
        added,
        removed,
        has_regression,
    };

    BaselineDiff {
        baseline_latest_ledger: baseline.latest_ledger,
        current_latest_ledger: current.latest_ledger,
        ledgers_elapsed,
        summary,
        entries: entry_diffs,
    }
}

fn status_severity(level: RiskLevel) -> u8 {
    match level {
        RiskLevel::Ok => 0,
        RiskLevel::Warn => 1,
        RiskLevel::Critical => 2,
        RiskLevel::Archived => 3,
    }
}

fn parse_risk_level(s: &str) -> RiskLevel {
    match s.to_ascii_uppercase().as_str() {
        "OK" => RiskLevel::Ok,
        "WARN" => RiskLevel::Warn,
        "CRITICAL" => RiskLevel::Critical,
        "ARCHIVED" => RiskLevel::Archived,
        _ => RiskLevel::Ok,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Durability, EntryType};
    use crate::usecases::StorageEntryReport;

    fn make_test_report(
        contract_id: &str,
        latest_ledger: u32,
        entries: Vec<StorageEntryReport>,
    ) -> TTLReport {
        TTLReport {
            contract_id: contract_id.to_string(),
            network: "testnet".to_string(),
            latest_ledger,
            ledger_velocity_seconds: 5.0,
            entries,
            overall_status: RiskLevel::Ok,
            archival_recommendations: Vec::new(),
            sac_asset_info: None,
            contract_code_hash: None,
            contract_bytecode_size: None,
            contract_upgrade: None,
        }
    }

    fn make_test_entry(
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

    #[test]
    fn test_category_extended() {
        let current_entry = make_test_entry("instance", 6_000_000, 500_000, RiskLevel::Ok);
        let current = make_test_report("CAAA", 5_500_000, vec![current_entry]);

        let baseline = BaselineSnapshot {
            contract_id: "CAAA".to_string(),
            network: "testnet".to_string(),
            latest_ledger: 5_400_000,
            entries: vec![BaselineEntrySnapshot {
                identifier: "instance".to_string(),
                live_until_ledger: 5_800_000,
                remaining_ledgers: 400_000,
                status: "ok".to_string(),
            }],
        };

        let diff = compare_reports(&current, &baseline);
        assert_eq!(diff.entries.len(), 1);
        assert_eq!(diff.entries[0].category, ChangeCategory::Extended);
        assert_eq!(diff.entries[0].live_until_delta, 200_000);
        assert_eq!(diff.ledgers_elapsed, 100_000);
        assert_eq!(diff.summary.extended, 1);
        assert!(!diff.summary.has_regression);
    }

    #[test]
    fn test_category_unchanged() {
        let current_entry = make_test_entry("instance", 5_800_000, 400_000, RiskLevel::Ok);
        let current = make_test_report("CAAA", 5_400_000, vec![current_entry]);

        let baseline = BaselineSnapshot {
            contract_id: "CAAA".to_string(),
            network: "testnet".to_string(),
            latest_ledger: 5_400_000,
            entries: vec![BaselineEntrySnapshot {
                identifier: "instance".to_string(),
                live_until_ledger: 5_800_000,
                remaining_ledgers: 400_000,
                status: "ok".to_string(),
            }],
        };

        let diff = compare_reports(&current, &baseline);
        assert_eq!(diff.entries[0].category, ChangeCategory::Unchanged);
        assert_eq!(diff.entries[0].live_until_delta, 0);
        assert_eq!(diff.ledgers_elapsed, 0);
        assert_eq!(diff.summary.unchanged, 1);
    }

    #[test]
    fn test_category_decayed() {
        let current_entry = make_test_entry("instance", 5_800_000, 390_000, RiskLevel::Ok);
        let current = make_test_report("CAAA", 5_410_000, vec![current_entry]);

        let baseline = BaselineSnapshot {
            contract_id: "CAAA".to_string(),
            network: "testnet".to_string(),
            latest_ledger: 5_400_000,
            entries: vec![BaselineEntrySnapshot {
                identifier: "instance".to_string(),
                live_until_ledger: 5_800_000,
                remaining_ledgers: 400_000,
                status: "ok".to_string(),
            }],
        };

        let diff = compare_reports(&current, &baseline);
        assert_eq!(diff.entries[0].category, ChangeCategory::Decayed);
        assert_eq!(diff.entries[0].live_until_delta, 0);
        assert_eq!(diff.ledgers_elapsed, 10_000);
        assert_eq!(diff.summary.decayed, 1);
    }

    #[test]
    fn test_category_status_worsened() {
        let current_entry = make_test_entry("instance", 5_500_000, 80_000, RiskLevel::Warn);
        let current = make_test_report("CAAA", 5_420_000, vec![current_entry]);

        let baseline = BaselineSnapshot {
            contract_id: "CAAA".to_string(),
            network: "testnet".to_string(),
            latest_ledger: 5_400_000,
            entries: vec![BaselineEntrySnapshot {
                identifier: "instance".to_string(),
                live_until_ledger: 5_500_000,
                remaining_ledgers: 100_000,
                status: "ok".to_string(),
            }],
        };

        let diff = compare_reports(&current, &baseline);
        assert_eq!(diff.entries[0].category, ChangeCategory::StatusWorsened);
        assert_eq!(diff.summary.status_worsened, 1);
        assert!(diff.summary.has_regression);
    }

    #[test]
    fn test_category_status_improved() {
        let current_entry = make_test_entry("instance", 5_800_000, 380_000, RiskLevel::Ok);
        let current = make_test_report("CAAA", 5_420_000, vec![current_entry]);

        let baseline = BaselineSnapshot {
            contract_id: "CAAA".to_string(),
            network: "testnet".to_string(),
            latest_ledger: 5_400_000,
            entries: vec![BaselineEntrySnapshot {
                identifier: "instance".to_string(),
                live_until_ledger: 5_500_000,
                remaining_ledgers: 100_000,
                status: "warn".to_string(),
            }],
        };

        let diff = compare_reports(&current, &baseline);
        assert_eq!(diff.entries[0].category, ChangeCategory::Extended); // live_until increased takes precedence
    }

    #[test]
    fn test_category_status_improved_same_live_until() {
        // If threshold configuration was modified to be less strict
        let current_entry = make_test_entry("instance", 5_500_000, 100_000, RiskLevel::Ok);
        let current = make_test_report("CAAA", 5_400_000, vec![current_entry]);

        let baseline = BaselineSnapshot {
            contract_id: "CAAA".to_string(),
            network: "testnet".to_string(),
            latest_ledger: 5_400_000,
            entries: vec![BaselineEntrySnapshot {
                identifier: "instance".to_string(),
                live_until_ledger: 5_500_000,
                remaining_ledgers: 100_000,
                status: "warn".to_string(),
            }],
        };

        let diff = compare_reports(&current, &baseline);
        assert_eq!(diff.entries[0].category, ChangeCategory::StatusImproved);
        assert_eq!(diff.summary.status_improved, 1);
    }

    #[test]
    fn test_category_added_and_removed() {
        let current_entry = make_test_entry("entry_new", 5_600_000, 200_000, RiskLevel::Ok);
        let current = make_test_report("CAAA", 5_400_000, vec![current_entry]);

        let baseline = BaselineSnapshot {
            contract_id: "CAAA".to_string(),
            network: "testnet".to_string(),
            latest_ledger: 5_400_000,
            entries: vec![BaselineEntrySnapshot {
                identifier: "entry_old".to_string(),
                live_until_ledger: 5_500_000,
                remaining_ledgers: 100_000,
                status: "ok".to_string(),
            }],
        };

        let diff = compare_reports(&current, &baseline);
        assert_eq!(diff.entries.len(), 2);
        assert_eq!(diff.summary.added, 1);
        assert_eq!(diff.summary.removed, 1);
        assert!(diff.summary.has_regression); // removed triggers has_regression
    }
}
