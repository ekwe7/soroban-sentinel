//! Pure domain detector for accelerated storage rent burn across two scan snapshots.
//!
//! Soroban entries decay by exactly one remaining ledger per closed ledger. When
//! two scans of the same contract are compared, any entry whose remaining
//! lifetime shrinks by more than the elapsed ledgers is burning rent faster than
//! natural time decay explains (for example after state expansion). This module
//! quantifies byte expansion and decay velocity per entry and flags
//! [`AcceleratedDecayWarning`]s. It performs no I/O.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::domain::ttl::TARGET_LEDGER_CLOSE_SECONDS;
use crate::usecases::{StorageEntryReport, TTLReport};

/// Default tolerance: remaining lifetime may shrink up to 10% faster than the
/// elapsed ledgers before an entry is flagged.
pub const DEFAULT_ACCELERATION_TOLERANCE_RATIO: f64 = 0.10;

/// Errors raised when comparing two scan reports.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum AccelerationError {
    /// The reports describe different contracts.
    #[error("Contract mismatch: baseline {baseline} vs current {current}")]
    ContractMismatch {
        /// Baseline contract id.
        baseline: String,
        /// Current contract id.
        current: String,
    },
    /// The reports were taken on different networks.
    #[error("Network mismatch: baseline {baseline} vs current {current}")]
    NetworkMismatch {
        /// Baseline network.
        baseline: String,
        /// Current network.
        current: String,
    },
    /// The current report is not strictly newer than the baseline.
    #[error("Current ledger {current} must be greater than baseline ledger {baseline}")]
    NonAdvancingLedgers {
        /// Baseline latest ledger.
        baseline: u32,
        /// Current latest ledger.
        current: u32,
    },
    /// The tolerance ratio is negative or not finite.
    #[error("Tolerance ratio must be finite and non-negative (got {0})")]
    InvalidTolerance(f64),
}

/// How an entry's remaining lifetime moved relative to natural decay.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecayClassification {
    /// Remaining lifetime shrank by about one ledger per elapsed ledger.
    Steady,
    /// Remaining lifetime shrank faster than elapsed ledgers beyond tolerance.
    Accelerated,
    /// The live-until ledger was extended, so decay is not comparable.
    Extended,
}

/// Direction of an entry's byte size between snapshots.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StateGrowth {
    /// The entry grew.
    Expanded,
    /// The entry shrank.
    Shrunk,
    /// The entry size is identical.
    Unchanged,
}

/// Acceleration metrics for one entry present in both snapshots.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EntryAcceleration {
    /// Entry key identifier.
    pub identifier: String,
    /// Decay classification against natural time decay.
    pub decay: DecayClassification,
    /// Byte size direction.
    pub growth: StateGrowth,
    /// Current bytes minus baseline bytes.
    pub byte_delta: i64,
    /// Bytes gained per elapsed ledger (negative when the entry shrank).
    pub byte_expansion_rate: f64,
    /// Current remaining ledgers minus baseline remaining ledgers.
    pub remaining_ledgers_delta: i64,
    /// Ledgers of remaining lifetime lost between snapshots.
    pub observed_decay_ledgers: i64,
    /// Ledgers expected to decay naturally (one per elapsed ledger).
    pub expected_decay_ledgers: i64,
    /// Observed decay minus expected decay (positive means faster burn).
    pub excess_decay_ledgers: i64,
    /// Observed change in remaining ledgers per second.
    pub decay_velocity: f64,
    /// Observed decay velocity minus natural decay velocity (negative means faster burn).
    pub decay_velocity_delta: f64,
}

/// Warning raised when an entry burns rent faster than natural decay.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AcceleratedDecayWarning {
    /// Entry key identifier.
    pub identifier: String,
    /// Ledgers expected to decay naturally.
    pub expected_decay_ledgers: i64,
    /// Ledgers of remaining lifetime actually lost.
    pub observed_decay_ledgers: i64,
    /// Observed minus expected decay.
    pub excess_decay_ledgers: i64,
    /// Bytes gained per elapsed ledger for this entry.
    pub byte_expansion_rate: f64,
}

/// Result of comparing two scan reports of the same contract.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AccelerationReport {
    /// Contract address strkey.
    pub contract_id: String,
    /// Network inspected.
    pub network: String,
    /// Latest ledger of the baseline scan.
    pub baseline_ledger: u32,
    /// Latest ledger of the current scan.
    pub current_ledger: u32,
    /// Ledgers elapsed between scans.
    pub ledgers_elapsed: i64,
    /// Wall-clock seconds elapsed, using the current scan's ledger velocity.
    pub elapsed_seconds: f64,
    /// Tolerance ratio applied when flagging accelerated decay.
    pub tolerance_ratio: f64,
    /// Total bytes across all entries, current minus baseline.
    pub total_byte_delta: i64,
    /// Aggregate bytes gained per elapsed ledger, including added and removed entries.
    pub aggregate_byte_expansion_rate: f64,
    /// Per-entry metrics for entries present in both snapshots.
    pub entries: Vec<EntryAcceleration>,
    /// Identifiers present only in the current scan (active entry creation).
    pub added_entries: Vec<String>,
    /// Identifiers present only in the baseline scan.
    pub removed_entries: Vec<String>,
    /// Entries burning rent faster than natural decay.
    pub warnings: Vec<AcceleratedDecayWarning>,
}

impl AccelerationReport {
    /// Returns true when at least one entry is flagged.
    pub fn has_accelerated_decay(&self) -> bool {
        !self.warnings.is_empty()
    }
}

/// Compares two scan reports taken at different ledger sequences.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StorageAccelerationTracker {
    tolerance_ratio: f64,
}

impl Default for StorageAccelerationTracker {
    fn default() -> Self {
        Self {
            tolerance_ratio: DEFAULT_ACCELERATION_TOLERANCE_RATIO,
        }
    }
}

#[derive(Clone, Copy)]
struct ElapsedWindow {
    ledgers: i64,
    seconds: f64,
    natural_decay_velocity: f64,
}

impl StorageAccelerationTracker {
    /// Creates a tracker with a custom tolerance ratio (0.0 flags any excess decay).
    pub fn new(tolerance_ratio: f64) -> Result<Self, AccelerationError> {
        if !tolerance_ratio.is_finite() || tolerance_ratio < 0.0 {
            return Err(AccelerationError::InvalidTolerance(tolerance_ratio));
        }
        Ok(Self { tolerance_ratio })
    }

    /// Returns the configured tolerance ratio.
    pub fn tolerance_ratio(&self) -> f64 {
        self.tolerance_ratio
    }

    /// Compares `current` against an earlier `baseline` scan of the same contract.
    pub fn compare(
        &self,
        baseline: &TTLReport,
        current: &TTLReport,
    ) -> Result<AccelerationReport, AccelerationError> {
        if baseline.contract_id != current.contract_id {
            return Err(AccelerationError::ContractMismatch {
                baseline: baseline.contract_id.clone(),
                current: current.contract_id.clone(),
            });
        }
        if baseline.network != current.network {
            return Err(AccelerationError::NetworkMismatch {
                baseline: baseline.network.clone(),
                current: current.network.clone(),
            });
        }
        if current.latest_ledger <= baseline.latest_ledger {
            return Err(AccelerationError::NonAdvancingLedgers {
                baseline: baseline.latest_ledger,
                current: current.latest_ledger,
            });
        }

        let ledgers = i64::from(current.latest_ledger) - i64::from(baseline.latest_ledger);
        let seconds_per_ledger = if current.ledger_velocity_seconds.is_finite()
            && current.ledger_velocity_seconds > 0.0
        {
            current.ledger_velocity_seconds
        } else {
            TARGET_LEDGER_CLOSE_SECONDS
        };
        let window = ElapsedWindow {
            ledgers,
            seconds: ledgers as f64 * seconds_per_ledger,
            natural_decay_velocity: -1.0 / seconds_per_ledger,
        };

        let baseline_by_id: HashMap<&str, &StorageEntryReport> = baseline
            .entries
            .iter()
            .map(|entry| (entry.key_identifier.as_str(), entry))
            .collect();
        let current_ids: HashSet<&str> = current
            .entries
            .iter()
            .map(|entry| entry.key_identifier.as_str())
            .collect();

        let mut entries = Vec::new();
        let mut added_entries = Vec::new();
        let mut warnings = Vec::new();

        for entry in &current.entries {
            match baseline_by_id.get(entry.key_identifier.as_str()) {
                Some(base) => {
                    let analysis = self.analyze_entry(base, entry, window);
                    if analysis.decay == DecayClassification::Accelerated {
                        warnings.push(AcceleratedDecayWarning {
                            identifier: analysis.identifier.clone(),
                            expected_decay_ledgers: analysis.expected_decay_ledgers,
                            observed_decay_ledgers: analysis.observed_decay_ledgers,
                            excess_decay_ledgers: analysis.excess_decay_ledgers,
                            byte_expansion_rate: analysis.byte_expansion_rate,
                        });
                    }
                    entries.push(analysis);
                }
                None => added_entries.push(entry.key_identifier.clone()),
            }
        }

        let removed_entries = baseline
            .entries
            .iter()
            .filter(|entry| !current_ids.contains(entry.key_identifier.as_str()))
            .map(|entry| entry.key_identifier.clone())
            .collect();

        let total_byte_delta = signed_delta(total_bytes(current), total_bytes(baseline));

        Ok(AccelerationReport {
            contract_id: current.contract_id.clone(),
            network: current.network.clone(),
            baseline_ledger: baseline.latest_ledger,
            current_ledger: current.latest_ledger,
            ledgers_elapsed: window.ledgers,
            elapsed_seconds: window.seconds,
            tolerance_ratio: self.tolerance_ratio,
            total_byte_delta,
            aggregate_byte_expansion_rate: total_byte_delta as f64 / window.ledgers as f64,
            entries,
            added_entries,
            removed_entries,
            warnings,
        })
    }

    fn analyze_entry(
        &self,
        base: &StorageEntryReport,
        cur: &StorageEntryReport,
        window: ElapsedWindow,
    ) -> EntryAcceleration {
        let byte_delta = signed_delta(cur.byte_size as u64, base.byte_size as u64);
        let growth = match byte_delta {
            d if d > 0 => StateGrowth::Expanded,
            d if d < 0 => StateGrowth::Shrunk,
            _ => StateGrowth::Unchanged,
        };

        let remaining_ledgers_delta = cur.remaining_ledgers.saturating_sub(base.remaining_ledgers);
        let observed_decay_ledgers = base.remaining_ledgers.saturating_sub(cur.remaining_ledgers);
        let expected_decay_ledgers = window.ledgers;
        let excess_decay_ledgers = observed_decay_ledgers.saturating_sub(expected_decay_ledgers);
        let decay_velocity = remaining_ledgers_delta as f64 / window.seconds;

        let extended = cur.live_until_ledger > base.live_until_ledger;
        let tolerance_ledgers = window.ledgers as f64 * self.tolerance_ratio;
        let decay = if extended {
            DecayClassification::Extended
        } else if excess_decay_ledgers as f64 > tolerance_ledgers {
            DecayClassification::Accelerated
        } else {
            DecayClassification::Steady
        };

        EntryAcceleration {
            identifier: cur.key_identifier.clone(),
            decay,
            growth,
            byte_delta,
            byte_expansion_rate: byte_delta as f64 / window.ledgers as f64,
            remaining_ledgers_delta,
            observed_decay_ledgers,
            expected_decay_ledgers,
            excess_decay_ledgers,
            decay_velocity,
            decay_velocity_delta: decay_velocity - window.natural_decay_velocity,
        }
    }
}

fn total_bytes(report: &TTLReport) -> u64 {
    report.entries.iter().fold(0u64, |acc, entry| {
        acc.saturating_add(entry.byte_size as u64)
    })
}

fn signed_delta(current: u64, baseline: u64) -> i64 {
    (i128::from(current) - i128::from(baseline)).clamp(i128::from(i64::MIN), i128::from(i64::MAX))
        as i64
}
