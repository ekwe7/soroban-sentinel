//! Entry size delta and storage bloat regression detection.
//!
//! Compares contract storage entry sizes across consecutive scan snapshots and
//! flags entries whose growth looks like a storage bloat regression. Results
//! feed the existing risk assessment pipeline: an entry approaching the
//! network maximum entry size is classified [`RiskLevel::Critical`], while
//! entries crossing the configured growth thresholds are classified
//! [`RiskLevel::Warn`].

use crate::domain::{evaluate_risk, RiskLevel, ThresholdConfig};
use crate::usecases::TTLReport;

/// Default network maximum ledger entry size in bytes (Soroban ledger entry limit).
pub const DEFAULT_MAX_ENTRY_SIZE_BYTES: usize = 65_536;

/// Default percentage growth threshold (entries growing more than 20% are flagged).
pub const DEFAULT_GROWTH_PERCENTAGE_THRESHOLD: f64 = 20.0;

/// Default absolute growth threshold in bytes (entries growing more than 1 KB are flagged).
pub const DEFAULT_ABSOLUTE_GROWTH_THRESHOLD_BYTES: usize = 1_024;

/// Default fraction of the maximum entry size at or above which a `Critical`
/// warning is emitted (90%).
pub const DEFAULT_MAX_SIZE_CRITICAL_RATIO: f64 = 0.90;

/// Errors produced by the bloat detector configuration.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum BloatError {
    #[error("percentage growth threshold must be greater than 0, got {0}")]
    InvalidPercentageThreshold(f64),
    #[error("absolute growth threshold must be greater than 0, got {0}")]
    InvalidAbsoluteThreshold(usize),
    #[error("maximum entry size must be greater than 0, got {0}")]
    InvalidMaxEntrySize(usize),
    #[error("max size critical ratio must be in (0.0, 1.0], got {0}")]
    InvalidMaxSizeCriticalRatio(f64),
}

/// Configuration for storage bloat detection.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BloatConfig {
    /// Percentage growth above which an entry is flagged (e.g. 20.0 means > 20%).
    pub percentage_growth_threshold: f64,
    /// Absolute byte growth above which an entry is flagged (e.g. 1024 means > 1 KB).
    pub absolute_growth_threshold_bytes: usize,
    /// Network maximum entry size in bytes.
    pub max_entry_size_bytes: usize,
    /// Fraction of `max_entry_size_bytes` at or above which the risk is `Critical`.
    pub max_size_critical_ratio: f64,
}

impl Default for BloatConfig {
    fn default() -> Self {
        Self {
            percentage_growth_threshold: DEFAULT_GROWTH_PERCENTAGE_THRESHOLD,
            absolute_growth_threshold_bytes: DEFAULT_ABSOLUTE_GROWTH_THRESHOLD_BYTES,
            max_entry_size_bytes: DEFAULT_MAX_ENTRY_SIZE_BYTES,
            max_size_critical_ratio: DEFAULT_MAX_SIZE_CRITICAL_RATIO,
        }
    }
}

impl BloatConfig {
    /// Constructs and validates a new `BloatConfig`.
    pub fn try_new(
        percentage_growth_threshold: f64,
        absolute_growth_threshold_bytes: usize,
        max_entry_size_bytes: usize,
        max_size_critical_ratio: f64,
    ) -> Result<Self, BloatError> {
        let config = Self {
            percentage_growth_threshold,
            absolute_growth_threshold_bytes,
            max_entry_size_bytes,
            max_size_critical_ratio,
        };
        config.validate()?;
        Ok(config)
    }

    /// Validates that the thresholds are logically sound.
    pub fn validate(&self) -> Result<(), BloatError> {
        if !self.percentage_growth_threshold.is_finite() || self.percentage_growth_threshold <= 0.0 {
            return Err(BloatError::InvalidPercentageThreshold(
                self.percentage_growth_threshold,
            ));
        }
        if self.absolute_growth_threshold_bytes == 0 {
            return Err(BloatError::InvalidAbsoluteThreshold(
                self.absolute_growth_threshold_bytes,
            ));
        }
        if self.max_entry_size_bytes == 0 {
            return Err(BloatError::InvalidMaxEntrySize(self.max_entry_size_bytes));
        }
        if !(0.0 < self.max_size_critical_ratio && self.max_size_critical_ratio <= 1.0) {
            return Err(BloatError::InvalidMaxSizeCriticalRatio(
                self.max_size_critical_ratio,
            ));
        }
        Ok(())
    }
}

/// Report of a storage entry size comparison between two snapshots.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BloatReport {
    /// Associated storage key identifier.
    pub storage_key: String,
    /// Entry size in the baseline (previous) snapshot.
    pub previous_entry_size: usize,
    /// Entry size in the current snapshot.
    pub current_entry_size: usize,
    /// Absolute byte growth (`current - previous`; negative when shrinking).
    pub absolute_growth: i64,
    /// Absolute value of `absolute_growth` in bytes.
    pub absolute_growth_bytes: usize,
    /// Percentage growth (`(current - previous) / previous * 100`). Zero when
    /// there is no baseline size to compare against.
    pub percentage_growth: f64,
    /// Risk level assigned to this entry.
    pub risk_level: RiskLevel,
    /// True when percentage growth crosses the configured threshold.
    pub exceeds_percentage_threshold: bool,
    /// True when absolute byte growth crosses the configured threshold.
    pub exceeds_absolute_threshold: bool,
    /// True when the current size is at or above the critical fraction of the
    /// network maximum entry size.
    pub approaching_max_size: bool,
}

impl BloatReport {
    /// True when the entry grew in size between snapshots.
    pub fn is_growing(&self) -> bool {
        self.current_entry_size > self.previous_entry_size
    }

    /// True when the entry should be flagged for review (any threshold breach
    /// or impending maximum size limit).
    pub fn is_flagged(&self) -> bool {
        self.exceeds_percentage_threshold
            || self.exceeds_absolute_threshold
            || self.approaching_max_size
    }
}

/// Storage bloat regression detector.
#[derive(Debug, Clone)]
pub struct BloatDetector {
    config: BloatConfig,
}

impl Default for BloatDetector {
    fn default() -> Self {
        Self::new(BloatConfig::default())
    }
}

impl BloatDetector {
    /// Creates a detector with the given configuration.
    pub fn new(config: BloatConfig) -> Self {
        Self { config }
    }

    /// Returns the detector configuration.
    pub fn config(&self) -> &BloatConfig {
        &self.config
    }

    /// Compares a single entry's size across two snapshots and returns its
    /// [`BloatReport`].
    ///
    /// When `previous_entry_size` is zero (no baseline data), no growth signal
    /// is emitted: percentage growth is undefined and absolute growth is not
    /// flagged, because there is nothing to regress against.
    pub fn analyze(
        &self,
        storage_key: impl Into<String>,
        previous_entry_size: usize,
        current_entry_size: usize,
    ) -> BloatReport {
        let storage_key = storage_key.into();
        let absolute_growth = current_entry_size as i64 - previous_entry_size as i64;
        let absolute_growth_bytes = absolute_growth.unsigned_abs() as usize;

        let percentage_growth = if previous_entry_size > 0 {
            absolute_growth as f64 / previous_entry_size as f64 * 100.0
        } else {
            0.0
        };

        let exceeds_percentage_threshold =
            previous_entry_size > 0 && percentage_growth > self.config.percentage_growth_threshold;
        let exceeds_absolute_threshold =
            previous_entry_size > 0 && absolute_growth > 0 && absolute_growth_bytes > self.config.absolute_growth_threshold_bytes;

        let max_size_threshold =
            (self.config.max_entry_size_bytes as f64 * self.config.max_size_critical_ratio) as usize;
        let approaching_max_size = current_entry_size >= max_size_threshold;

        let risk_level = if approaching_max_size {
            RiskLevel::Critical
        } else if exceeds_percentage_threshold {
            RiskLevel::Warn
        } else {
            RiskLevel::Ok
        };

        BloatReport {
            storage_key,
            previous_entry_size,
            current_entry_size,
            absolute_growth,
            absolute_growth_bytes,
            percentage_growth,
            risk_level,
            exceeds_percentage_threshold,
            exceeds_absolute_threshold,
            approaching_max_size,
        }
    }

    /// Compares every entry of a current [`TTLReport`] against a baseline
    /// snapshot, matching entries by storage key identifier, and returns one
    /// [`BloatReport`] per comparable entry. Entries without a baseline size
    /// are skipped.
    pub fn detect_from_baseline(
        &self,
        current: &TTLReport,
        baseline: &crate::domain::compare::BaselineSnapshot,
    ) -> Vec<BloatReport> {
        let mut reports = Vec::with_capacity(current.entries.len());
        for entry in &current.entries {
            let previous_size = baseline
                .entries
                .iter()
                .find(|baseline_entry| baseline_entry.identifier == entry.key_identifier)
                .map(|baseline_entry| baseline_entry.byte_size)
                .unwrap_or(0);
            if previous_size == 0 {
                continue;
            }
            reports.push(self.analyze(&entry.key_identifier, previous_size, entry.byte_size));
        }
        reports
    }

    /// Projects remaining ledgers accounting for entry size growth. Rent burn
    /// scales with entry size, so an entry that grew has proportionally fewer
    /// ledgers remaining than its raw live-until ledger suggests:
    ///
    /// `adjusted = remaining_ledgers * previous_size / current_size`
    ///
    /// Returns `remaining_ledgers` unchanged when there is no baseline size or
    /// when no meaningful adjustment can be made.
    pub fn adjusted_remaining_ledgers(
        remaining_ledgers: i64,
        previous_entry_size: usize,
        current_entry_size: usize,
    ) -> i64 {
        if remaining_ledgers <= 0
            || previous_entry_size == 0
            || current_entry_size == 0
        {
            return remaining_ledgers;
        }
        let ratio = previous_entry_size as f64 / current_entry_size as f64;
        let adjusted = remaining_ledgers as f64 * ratio;
        if adjusted.is_finite() && adjusted > 0.0 && adjusted < i64::MAX as f64 {
            adjusted as i64
        } else {
            remaining_ledgers
        }
    }

    /// Re-evaluates an entry's risk status taking storage bloat into account.
    ///
    /// The returned risk is the most severe of: the entry's existing status,
    /// the risk implied by the size-adjusted remaining ledgers, and the
    /// bloat report risk level.
    pub fn assess_entry_risk(
        status: RiskLevel,
        remaining_ledgers: i64,
        previous_entry_size: usize,
        current_entry_size: usize,
        threshold_config: &ThresholdConfig,
    ) -> RiskLevel {
        let report = Self::default().analyze("", previous_entry_size, current_entry_size);
        let adjusted_ledgers =
            Self::adjusted_remaining_ledgers(remaining_ledgers, previous_entry_size, current_entry_size);
        let size_risk = evaluate_risk(adjusted_ledgers, threshold_config);
        severity_max(severity_max(status, size_risk), report.risk_level)
    }
}

/// Severity ordering: `Ok < Warn < Critical < Archived`.
fn severity_max(a: RiskLevel, b: RiskLevel) -> RiskLevel {
    if a as u8 >= b as u8 {
        a
    } else {
        b
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_config() -> BloatConfig {
        BloatConfig::default()
    }

    #[test]
    fn test_no_storage_growth() {
        let detector = BloatDetector::new(default_config());
        let report = detector.analyze("counter", 512, 512);
        assert_eq!(report.absolute_growth, 0);
        assert_eq!(report.absolute_growth_bytes, 0);
        assert_eq!(report.percentage_growth, 0.0);
        assert!(!report.is_growing());
        assert!(!report.is_flagged());
        assert_eq!(report.risk_level, RiskLevel::Ok);
    }

    #[test]
    fn test_gradual_storage_expansion() {
        let detector = BloatDetector::new(default_config());
        let sizes = [512, 560, 620, 700];
        let mut previous = sizes[0];
        let mut flagged = 0;
        for &current in &sizes[1..] {
            let report = detector.analyze("counter", previous, current);
            previous = current;
            if report.is_flagged() {
                flagged += 1;
            }
        }
        // Gradual expansion stays under both thresholds until it does not.
        assert_eq!(flagged, 0);
        // Check percentage growth from 620 to 700 (the last transition).
        let report = detector.analyze("counter", 620, 700);
        assert!((report.percentage_growth - (700.0 / 620.0 - 1.0) * 100.0).abs() < 1e-6);
    }

    #[test]
    fn test_sudden_storage_expansion() {
        let detector = BloatDetector::new(default_config());
        // Doubling in size: > 20% growth -> Warn.
        let report = detector.analyze("counter", 1000, 2500);
        assert_eq!(report.absolute_growth, 1500);
        assert!((report.percentage_growth - 150.0).abs() < 1e-6);
        assert!(report.exceeds_percentage_threshold);
        assert_eq!(report.risk_level, RiskLevel::Warn);
        assert!(report.is_flagged());
    }

    #[test]
    fn test_percentage_growth_calculation() {
        let detector = BloatDetector::new(default_config());
        let report = detector.analyze("key", 1000, 1250);
        assert!((report.percentage_growth - 25.0).abs() < 1e-6);
        assert!(report.exceeds_percentage_threshold);

        let report = detector.analyze("key", 1000, 1200);
        assert!((report.percentage_growth - 20.0).abs() < 1e-6);
        // "More than 20%" -> exactly 20% is not flagged.
        assert!(!report.exceeds_percentage_threshold);
        assert_eq!(report.risk_level, RiskLevel::Ok);
    }

    #[test]
    fn test_absolute_byte_growth_threshold() {
        let detector = BloatDetector::new(default_config());
        // Exactly 1 KB growth is not "more than 1 KB".
        let report = detector.analyze("key", 1000, 2024);
        assert_eq!(report.absolute_growth_bytes, 1024);
        assert!(!report.exceeds_absolute_threshold);

        let report = detector.analyze("key", 1000, 2025);
        assert_eq!(report.absolute_growth_bytes, 1025);
        assert!(report.exceeds_absolute_threshold);
        assert_eq!(report.risk_level, RiskLevel::Warn);
    }

    #[test]
    fn test_shrinkage_is_not_flagged() {
        let detector = BloatDetector::new(default_config());
        let report = detector.analyze("key", 5000, 100);
        assert_eq!(report.absolute_growth, -4900);
        assert_eq!(report.absolute_growth_bytes, 4900);
        assert!(!report.exceeds_absolute_threshold);
        assert_eq!(report.risk_level, RiskLevel::Ok);
    }

    #[test]
    fn test_configurable_thresholds() {
        let config = BloatConfig {
            percentage_growth_threshold: 10.0,
            absolute_growth_threshold_bytes: 100,
            max_entry_size_bytes: 65_536,
            max_size_critical_ratio: 0.90,
        };
        let detector = BloatDetector::new(config);
        let report = detector.analyze("key", 1000, 1050);
        assert!((report.percentage_growth - 5.0).abs() < 1e-6);
        assert!(!report.exceeds_percentage_threshold);

        let report = detector.analyze("key", 1000, 1150);
        assert!(report.exceeds_percentage_threshold);
        assert_eq!(report.risk_level, RiskLevel::Warn);

        let report = detector.analyze("key", 1000, 1101);
        assert!(report.exceeds_absolute_threshold);
        assert_eq!(report.risk_level, RiskLevel::Warn);
    }

    #[test]
    fn test_critical_warning_at_90_percent_of_max_entry_size() {
        let detector = BloatDetector::new(default_config());
        let max = DEFAULT_MAX_ENTRY_SIZE_BYTES;
        let critical_threshold = (max as f64 * 0.90) as usize;
        assert_eq!(critical_threshold, 58_982);

        // Below 90%: not critical on size grounds alone.
        let below = detector.analyze("key", 50_000, critical_threshold - 1);
        assert!(!below.approaching_max_size);
        assert_eq!(below.risk_level, RiskLevel::Ok);

        // At 90%: critical.
        let at = detector.analyze("key", 50_000, critical_threshold);
        assert!(at.approaching_max_size);
        assert_eq!(at.risk_level, RiskLevel::Critical);

        // Above 90%: critical even without crossing growth thresholds.
        let above = detector.analyze("key", critical_threshold, max);
        assert!(above.approaching_max_size);
        assert_eq!(above.risk_level, RiskLevel::Critical);
    }

    #[test]
    fn test_critical_warning_overrides_growth_flags() {
        let detector = BloatDetector::new(default_config());
        // Small relative growth but close to the limit -> Critical.
        let report = detector.analyze("key", 58_000, 59_000);
        assert!(!report.exceeds_percentage_threshold);
        assert!(!report.exceeds_absolute_threshold);
        assert!(report.approaching_max_size);
        assert_eq!(report.risk_level, RiskLevel::Critical);
    }

    #[test]
    fn test_missing_baseline_produces_no_growth_signal() {
        let detector = BloatDetector::new(default_config());
        let report = detector.analyze("key", 0, 10_000);
        assert_eq!(report.percentage_growth, 0.0);
        assert!(!report.exceeds_absolute_threshold);
        assert!(!report.exceeds_percentage_threshold);
        assert_eq!(report.risk_level, RiskLevel::Ok);
    }

    #[test]
    fn test_invalid_configurations_are_rejected() {
        assert!(BloatConfig::try_new(0.0, 1024, 65_536, 0.9).is_err());
        assert!(BloatConfig::try_new(20.0, 0, 65_536, 0.9).is_err());
        assert!(BloatConfig::try_new(20.0, 1024, 0, 0.9).is_err());
        assert!(BloatConfig::try_new(20.0, 1024, 65_536, 0.0).is_err());
        assert!(BloatConfig::try_new(20.0, 1024, 65_536, 1.5).is_err());
        assert!(BloatConfig::try_new(20.0, 1024, 65_536, 0.9).is_ok());
    }

    #[test]
    fn test_adjusted_remaining_ledgers_with_inflated_entry_size() {
        // Doubling the entry size halves the projected remaining ledgers.
        let adjusted = BloatDetector::adjusted_remaining_ledgers(100_000, 100, 200);
        assert_eq!(adjusted, 50_000);

        // No baseline: no adjustment.
        assert_eq!(
            BloatDetector::adjusted_remaining_ledgers(100_000, 0, 200),
            100_000
        );

        // Shrinking entries project more remaining ledgers.
        let adjusted = BloatDetector::adjusted_remaining_ledgers(100_000, 200, 100);
        assert_eq!(adjusted, 200_000);

        // Expired entries stay at zero.
        assert_eq!(
            BloatDetector::adjusted_remaining_ledgers(0, 100, 200),
            0
        );
    }

    #[test]
    fn test_assess_entry_risk_escalation() {
        let threshold_config = ThresholdConfig::default();
        // Healthy entry that doubled in size: adjusted ledgers fall, risk escalates.
        let risk = BloatDetector::assess_entry_risk(
            RiskLevel::Ok,
            100_000,
            100,
            200,
            &threshold_config,
        );
        assert!(risk >= RiskLevel::Warn);

        // Critical bloat overrides an Ok status.
        let risk = BloatDetector::assess_entry_risk(
            RiskLevel::Ok,
            500_000,
            58_000,
            59_000,
            &threshold_config,
        );
        assert_eq!(risk, RiskLevel::Critical);

        // Archived stays Archived.
        let risk = BloatDetector::assess_entry_risk(
            RiskLevel::Archived,
            100_000,
            100,
            200,
            &threshold_config,
        );
        assert_eq!(risk, RiskLevel::Archived);
    }
}
