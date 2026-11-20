//! Cost-benefit analysis for letting live persistent entries expire.

use serde::{Deserialize, Serialize};

use super::{Durability, EntryType, StorageEntry};

/// Continuous TTL extension history for one entry on the scanned network.
#[derive(Debug, Clone, Copy)]
pub struct ExtensionHistory {
    pub first_ledger: u32,
    pub last_ledger: u32,
    pub extension_count: u32,
}

/// Eligibility thresholds and network-specific fee estimates in stroops.
#[derive(Debug, Clone, Copy)]
pub struct ArchivalPolicy {
    pub minimum_byte_size: usize,
    /// Minimum time since modification and minimum extension observation window.
    pub minimum_inactive_ledgers: u32,
    /// Number of 365-day years projected from the scan ledger.
    pub projection_years: u32,
    pub rent_stroops_per_kib_ledger: f64,
    /// Fixed restore transaction estimate, including minimum restored TTL rent.
    pub restoration_base_stroops: f64,
    /// Size-dependent restore fee estimate, including read and write costs.
    pub restoration_stroops_per_kib: f64,
}

/// Projected savings after reserving the cost of one restoration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArchivalRecommendation {
    pub key_identifier: String,
    pub byte_size: usize,
    pub ledgers_since_modification: u32,
    pub projected_renewal_ledgers: u64,
    pub rent_stroops_per_ledger: f64,
    pub projected_rent_stroops: f64,
    pub restoration_cost_stroops: f64,
    pub projected_savings_stroops: f64,
}

/// Analyze a live entry using explicit extension evidence and fee estimates.
pub fn recommend_archival(
    entry: &StorageEntry,
    history: ExtensionHistory,
    policy: &ArchivalPolicy,
    latest_ledger: u32,
    ledger_seconds: f64,
) -> Option<ArchivalRecommendation> {
    let age = latest_ledger.checked_sub(entry.last_modified_ledger())?;
    let observed_ledgers = latest_ledger.checked_sub(history.first_ledger)?;
    if entry.entry_type() != EntryType::PersistentData
        || entry.durability() != Durability::Persistent
        || entry.live_until_ledger() < latest_ledger
        || entry.live_until_ledger() == 0
        || entry.last_modified_ledger() == 0
        || history.first_ledger == 0
        || history.last_ledger < latest_ledger
        || policy.minimum_inactive_ledgers == 0
        || age < policy.minimum_inactive_ledgers
        || observed_ledgers < policy.minimum_inactive_ledgers
        || history.extension_count != 0
        || entry.byte_size() == 0
        || entry.byte_size() < policy.minimum_byte_size
        || policy.projection_years < 2
        || !ledger_seconds.is_finite()
        || ledger_seconds <= 0.0
        || [
            policy.rent_stroops_per_kib_ledger,
            policy.restoration_base_stroops,
            policy.restoration_stroops_per_kib,
        ]
        .iter()
        .any(|fee| !fee.is_finite() || *fee < 0.0)
    {
        return None;
    }

    let horizon = (f64::from(policy.projection_years) * 365.0 * super::SECONDS_PER_DAY
        / ledger_seconds)
        .floor();
    if horizon >= u64::MAX as f64 {
        return None;
    }
    // Rent already paid through the current TTL cannot be recovered.
    let prepaid_ledgers = u64::from(entry.live_until_ledger() - latest_ledger);
    let projected_renewal_ledgers = (horizon as u64).saturating_sub(prepaid_ledgers);
    let kib = entry.byte_size() as f64 / 1024.0;
    let rent_stroops_per_ledger = kib * policy.rent_stroops_per_kib_ledger;
    let projected_rent_stroops =
        (rent_stroops_per_ledger * projected_renewal_ledgers as f64).floor();
    let restoration_cost_stroops =
        (policy.restoration_base_stroops + kib * policy.restoration_stroops_per_kib).ceil();
    let projected_savings_stroops = projected_rent_stroops - restoration_cost_stroops;

    (projected_savings_stroops.is_finite() && projected_savings_stroops > 0.0).then(|| {
        ArchivalRecommendation {
            key_identifier: entry.key_identifier().to_owned(),
            byte_size: entry.byte_size(),
            ledgers_since_modification: age,
            projected_renewal_ledgers,
            rent_stroops_per_ledger,
            projected_rent_stroops,
            restoration_cost_stroops,
            projected_savings_stroops,
        }
    })
}
