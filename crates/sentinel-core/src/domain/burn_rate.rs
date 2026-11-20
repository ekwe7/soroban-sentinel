//! Pure domain mathematics for storage burn rate and dynamic TTL decay velocity.
//!
//! Soroban entries decay by one ledger per closed ledger. On real networks the
//! wall-clock time between ledger closes is irregular, so a fixed 5 second
//! assumption can materially misestimate the time until an entry expires. This
//! module turns a rolling window of ledger close timestamps into an observed
//! burn velocity and projects when remaining ledgers will be exhausted.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::domain::ttl::TARGET_LEDGER_CLOSE_SECONDS;

/// Default number of recent ledgers sampled to measure burn velocity.
pub const DEFAULT_SAMPLE_WINDOW_LEDGERS: u32 = 100;

/// Minimum supported sampling window.
pub const MIN_SAMPLE_WINDOW_LEDGERS: u32 = 2;

/// Maximum supported sampling window to bound in-memory sampling and RPC work.
pub const MAX_SAMPLE_WINDOW_LEDGERS: u32 = 10_000;

/// Lower plausibility bound for a measured ledger close interval in seconds.
const MIN_PLAUSIBLE_SECONDS_PER_LEDGER: f64 = 1.0;

/// Upper plausibility bound for a measured ledger close interval in seconds.
const MAX_PLAUSIBLE_SECONDS_PER_LEDGER: f64 = 60.0;

/// Errors raised when configuring burn rate sampling.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum BurnRateError {
    /// The requested sampling window is outside the supported bounds.
    #[error("Sample window must be between {min} and {max} ledgers (got {actual})")]
    InvalidSampleWindow {
        /// Requested window size.
        actual: u32,
        /// Minimum accepted window size.
        min: u32,
        /// Maximum accepted window size.
        max: u32,
    },
}

/// A single ledger close observation used to measure decay velocity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LedgerCloseSample {
    /// Ledger sequence number.
    pub sequence: u32,
    /// Ledger close time as Unix seconds.
    pub close_time: i64,
}

impl LedgerCloseSample {
    /// Create a new ledger close sample.
    pub fn new(sequence: u32, close_time: i64) -> Self {
        Self {
            sequence,
            close_time,
        }
    }
}

/// Origin of a computed burn rate measurement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BurnRateSource {
    /// Velocity was measured from supplied ledger close history.
    Measured,
    /// History was unavailable or invalid, so the network target velocity was used.
    Fallback,
}

impl BurnRateSource {
    /// Returns true when the value came from observed ledger history.
    pub fn is_measured(self) -> bool {
        matches!(self, Self::Measured)
    }
}

/// Observed storage burn velocity and sampling metadata.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BurnRate {
    /// Measured or fallback seconds per ledger close.
    pub seconds_per_ledger: f64,
    /// Ledgers consumed per second (the inverse of `seconds_per_ledger`).
    pub ledgers_per_second: f64,
    /// Number of valid close samples used for the measurement.
    pub sample_count: usize,
    /// Ledger sequence span covered by the valid samples.
    pub observed_ledgers: u32,
    /// Whether the velocity was measured or fell back to the target constant.
    pub source: BurnRateSource,
    /// Close time of the newest valid sample, when available.
    pub latest_close_time: Option<i64>,
}

impl Default for BurnRate {
    fn default() -> Self {
        Self::fallback()
    }
}

impl BurnRate {
    /// Creates the standard network target burn rate (5.0 seconds per ledger).
    pub fn fallback() -> Self {
        Self::from_validated(
            TARGET_LEDGER_CLOSE_SECONDS,
            0,
            0,
            BurnRateSource::Fallback,
            None,
        )
    }

    /// Creates a measured burn rate from a trusted seconds-per-ledger value.
    ///
    /// Values outside the plausible 1.0-60.0 second range fall back to the
    /// standard target velocity rather than producing nonsensical projections.
    pub fn measured(seconds_per_ledger: f64) -> Self {
        Self::from_validated(seconds_per_ledger, 0, 0, BurnRateSource::Measured, None)
    }

    fn from_validated(
        seconds_per_ledger: f64,
        sample_count: usize,
        observed_ledgers: u32,
        source: BurnRateSource,
        latest_close_time: Option<i64>,
    ) -> Self {
        let (seconds_per_ledger, source) = if is_plausible_velocity(seconds_per_ledger) {
            (seconds_per_ledger, source)
        } else {
            (TARGET_LEDGER_CLOSE_SECONDS, BurnRateSource::Fallback)
        };

        Self {
            seconds_per_ledger,
            ledgers_per_second: 1.0 / seconds_per_ledger,
            sample_count,
            observed_ledgers,
            source,
            latest_close_time,
        }
    }

    /// Returns true when the velocity was measured from ledger history.
    pub fn is_measured(&self) -> bool {
        self.source.is_measured()
    }
}

/// Wall-clock projection for exhausting a number of remaining ledgers.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ProjectedExhaustion {
    /// Remaining ledger count at projection time.
    pub remaining_ledgers: i64,
    /// Projected seconds until exhaustion.
    pub remaining_seconds: f64,
    /// Projected Unix timestamp of exhaustion, when a close time is available.
    pub exhaustion_timestamp: Option<i64>,
}

/// Validates a requested sampling window.
pub fn validate_sample_window(sample_window: u32) -> Result<u32, BurnRateError> {
    if !(MIN_SAMPLE_WINDOW_LEDGERS..=MAX_SAMPLE_WINDOW_LEDGERS).contains(&sample_window) {
        return Err(BurnRateError::InvalidSampleWindow {
            actual: sample_window,
            min: MIN_SAMPLE_WINDOW_LEDGERS,
            max: MAX_SAMPLE_WINDOW_LEDGERS,
        });
    }
    Ok(sample_window)
}

/// Calculates burn velocity from a rolling window of ledger close samples.
///
/// The velocity is fitted with an ordinary least-squares slope over
/// `(ledger_sequence, close_time)` pairs. This makes the result resilient to
/// irregular close intervals and uneven gaps in the sampled history. Invalid
/// samples (zero/negative timestamps or duplicate sequences), zero ledger
/// spans, and implausible or non-positive fitted slopes all fall back to the
/// supplied target velocity.
pub fn calculate_burn_rate(samples: &[LedgerCloseSample], fallback: f64) -> BurnRate {
    let mut valid: Vec<LedgerCloseSample> = samples
        .iter()
        .copied()
        .filter(|sample| sample.sequence > 0 && sample.close_time > 0)
        .collect();

    valid.sort_by_key(|sample| (sample.sequence, sample.close_time));
    valid.dedup_by_key(|sample| sample.sequence);

    let fallback_rate = BurnRate::from_validated(
        fallback,
        valid.len(),
        0,
        BurnRateSource::Fallback,
        valid.last().map(|sample| sample.close_time),
    );

    if valid.len() < 2 {
        return fallback_rate;
    }

    let first = valid[0];
    let last = valid[valid.len() - 1];
    if last.sequence <= first.sequence {
        return fallback_rate;
    }

    let observed_ledgers = last.sequence - first.sequence;
    let count = valid.len() as f64;
    let mean_sequence = valid
        .iter()
        .map(|sample| f64::from(sample.sequence))
        .sum::<f64>()
        / count;
    let mean_time = valid
        .iter()
        .map(|sample| sample.close_time as f64)
        .sum::<f64>()
        / count;

    let mut covariance = 0.0;
    let mut variance = 0.0;
    for sample in &valid {
        let sequence_delta = f64::from(sample.sequence) - mean_sequence;
        covariance += sequence_delta * (sample.close_time as f64 - mean_time);
        variance += sequence_delta * sequence_delta;
    }

    if variance <= 0.0 || !covariance.is_finite() || !variance.is_finite() {
        return fallback_rate;
    }

    let slope = covariance / variance;
    if !is_plausible_velocity(slope) {
        return fallback_rate;
    }

    BurnRate::from_validated(
        slope,
        valid.len(),
        observed_ledgers,
        BurnRateSource::Measured,
        Some(last.close_time),
    )
}

/// Projects the wall-clock exhaustion time for a number of remaining ledgers.
///
/// Returns `None` when the entry is already exhausted or the projection cannot
/// be represented as a finite duration.
pub fn project_exhaustion(
    burn_rate: &BurnRate,
    remaining_ledgers: i64,
    latest_close_time: Option<i64>,
) -> Option<ProjectedExhaustion> {
    if remaining_ledgers <= 0 {
        return None;
    }

    let remaining_seconds = (remaining_ledgers as f64) * burn_rate.seconds_per_ledger;
    if !remaining_seconds.is_finite() {
        return None;
    }

    let exhaustion_timestamp = latest_close_time.and_then(|timestamp| {
        let offset = remaining_seconds.ceil();
        if offset > i64::MAX as f64 {
            return None;
        }
        timestamp.checked_add(offset as i64)
    });

    Some(ProjectedExhaustion {
        remaining_ledgers,
        remaining_seconds,
        exhaustion_timestamp,
    })
}

fn is_plausible_velocity(seconds_per_ledger: f64) -> bool {
    seconds_per_ledger.is_finite()
        && (MIN_PLAUSIBLE_SECONDS_PER_LEDGER..=MAX_PLAUSIBLE_SECONDS_PER_LEDGER)
            .contains(&seconds_per_ledger)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn samples(pairs: &[(u32, i64)]) -> Vec<LedgerCloseSample> {
        pairs
            .iter()
            .map(|(sequence, close_time)| LedgerCloseSample::new(*sequence, *close_time))
            .collect()
    }

    #[test]
    fn regular_intervals_produce_measured_velocity() {
        let history = samples(&[(100, 1_000), (101, 1_005), (102, 1_010), (103, 1_015)]);
        let rate = calculate_burn_rate(&history, TARGET_LEDGER_CLOSE_SECONDS);

        assert_eq!(rate.source, BurnRateSource::Measured);
        assert!((rate.seconds_per_ledger - 5.0).abs() < 1e-9);
        assert!((rate.ledgers_per_second - 0.2).abs() < 1e-9);
        assert_eq!(rate.sample_count, 4);
        assert_eq!(rate.observed_ledgers, 3);
        assert_eq!(rate.latest_close_time, Some(1_015));
    }

    #[test]
    fn irregular_intervals_use_all_samples_without_skewing_to_latest_gap() {
        // Close gaps of 4s, 7s, 3s and 6s: the OLS slope is 5s/ledger.
        let history = samples(&[(1, 100), (2, 104), (3, 111), (4, 114), (5, 120)]);
        let rate = calculate_burn_rate(&history, TARGET_LEDGER_CLOSE_SECONDS);

        assert_eq!(rate.source, BurnRateSource::Measured);
        assert!((rate.seconds_per_ledger - 5.0).abs() < 1e-9);
        assert_eq!(rate.observed_ledgers, 4);
    }

    #[test]
    fn unsorted_samples_are_normalized() {
        let history = samples(&[(103, 1_015), (101, 1_005), (102, 1_010), (100, 1_000)]);
        let rate = calculate_burn_rate(&history, TARGET_LEDGER_CLOSE_SECONDS);

        assert_eq!(rate.source, BurnRateSource::Measured);
        assert!((rate.seconds_per_ledger - 5.0).abs() < 1e-9);
        assert_eq!(rate.observed_ledgers, 3);
    }

    #[test]
    fn duplicate_sequences_are_ignored() {
        let history = samples(&[(100, 1_000), (101, 1_005), (101, 1_006), (102, 1_010)]);
        let rate = calculate_burn_rate(&history, TARGET_LEDGER_CLOSE_SECONDS);

        assert_eq!(rate.sample_count, 3);
        assert_eq!(rate.observed_ledgers, 2);
        assert!((rate.seconds_per_ledger - 5.0).abs() < 1e-9);
    }

    #[test]
    fn zero_or_negative_close_times_are_invalid_and_fall_back() {
        let history = samples(&[(100, 0), (101, -5), (102, 0), (103, 10)]);
        let rate = calculate_burn_rate(&history, TARGET_LEDGER_CLOSE_SECONDS);

        assert_eq!(rate.source, BurnRateSource::Fallback);
        assert_eq!(rate.sample_count, 1);
        assert_eq!(rate.seconds_per_ledger, TARGET_LEDGER_CLOSE_SECONDS);
    }

    #[test]
    fn zero_time_interval_falls_back() {
        let history = samples(&[(100, 1_000), (101, 1_000), (102, 1_000)]);
        let rate = calculate_burn_rate(&history, TARGET_LEDGER_CLOSE_SECONDS);

        assert_eq!(rate.source, BurnRateSource::Fallback);
        assert_eq!(rate.seconds_per_ledger, TARGET_LEDGER_CLOSE_SECONDS);
    }

    #[test]
    fn decreasing_timestamps_fall_back() {
        let history = samples(&[(100, 1_000), (101, 995), (102, 990)]);
        let rate = calculate_burn_rate(&history, TARGET_LEDGER_CLOSE_SECONDS);

        assert_eq!(rate.source, BurnRateSource::Fallback);
        assert_eq!(rate.seconds_per_ledger, TARGET_LEDGER_CLOSE_SECONDS);
    }

    #[test]
    fn non_contiguous_ledgers_are_supported() {
        // Ledgers 10, 20 and 30 closed after 50s, 100s and 150s.
        let history = samples(&[(10, 1_000), (20, 1_050), (30, 1_100)]);
        let rate = calculate_burn_rate(&history, TARGET_LEDGER_CLOSE_SECONDS);

        assert_eq!(rate.source, BurnRateSource::Measured);
        assert!((rate.seconds_per_ledger - 5.0).abs() < 1e-9);
        assert_eq!(rate.observed_ledgers, 20);
    }

    #[test]
    fn insufficient_history_uses_fallback() {
        for history in [Vec::new(), samples(&[(100, 1_000)])] {
            let rate = calculate_burn_rate(&history, TARGET_LEDGER_CLOSE_SECONDS);
            assert_eq!(rate.source, BurnRateSource::Fallback);
            assert_eq!(rate.seconds_per_ledger, TARGET_LEDGER_CLOSE_SECONDS);
        }
    }

    #[test]
    fn implausible_measured_velocities_fall_back() {
        // 0.1s/ledger is too fast to be a real Stellar close interval.
        let fast = samples(&[(100, 1_000), (101, 1_000), (102, 1_000)]);
        let rate = calculate_burn_rate(&fast, TARGET_LEDGER_CLOSE_SECONDS);
        assert_eq!(rate.source, BurnRateSource::Fallback);

        // 120s/ledger is too slow for the accepted plausibility window.
        let slow = samples(&[(100, 1_000), (101, 1_120)]);
        let rate = calculate_burn_rate(&slow, TARGET_LEDGER_CLOSE_SECONDS);
        assert_eq!(rate.source, BurnRateSource::Fallback);
    }

    #[test]
    fn custom_fallback_velocity_is_preserved_when_history_is_unavailable() {
        let rate = calculate_burn_rate(&[], 6.5);
        assert_eq!(rate.source, BurnRateSource::Fallback);
        assert!((rate.seconds_per_ledger - 6.5).abs() < 1e-9);
        assert!((rate.ledgers_per_second - (1.0 / 6.5)).abs() < 1e-9);
    }

    #[test]
    fn invalid_fallback_velocity_normalizes_to_target() {
        for fallback in [f64::NAN, f64::INFINITY, 0.0, -5.0, 0.5, 120.0] {
            let rate = calculate_burn_rate(&[], fallback);
            assert_eq!(rate.seconds_per_ledger, TARGET_LEDGER_CLOSE_SECONDS);
            assert_eq!(rate.source, BurnRateSource::Fallback);
        }
    }

    #[test]
    fn projections_use_burn_rate_and_close_time() {
        let rate = BurnRate::measured(5.5);
        let projection = project_exhaustion(&rate, 1_000, Some(10_000)).unwrap();

        assert_eq!(projection.remaining_ledgers, 1_000);
        assert!((projection.remaining_seconds - 5_500.0).abs() < 1e-9);
        assert_eq!(projection.exhaustion_timestamp, Some(15_500));
    }

    #[test]
    fn projections_handle_exhausted_entries_and_missing_close_time() {
        let rate = BurnRate::default();
        assert!(project_exhaustion(&rate, 0, Some(10_000)).is_none());
        assert!(project_exhaustion(&rate, -10, Some(10_000)).is_none());

        let projection = project_exhaustion(&rate, 10, None).unwrap();
        assert_eq!(projection.remaining_seconds, 50.0);
        assert_eq!(projection.exhaustion_timestamp, None);
    }

    #[test]
    fn sample_window_validation_enforces_bounds() {
        assert_eq!(validate_sample_window(2).unwrap(), 2);
        assert_eq!(
            validate_sample_window(DEFAULT_SAMPLE_WINDOW_LEDGERS).unwrap(),
            DEFAULT_SAMPLE_WINDOW_LEDGERS
        );
        assert_eq!(
            validate_sample_window(MAX_SAMPLE_WINDOW_LEDGERS).unwrap(),
            MAX_SAMPLE_WINDOW_LEDGERS
        );

        for invalid in [0, 1, MAX_SAMPLE_WINDOW_LEDGERS + 1] {
            assert!(matches!(
                validate_sample_window(invalid),
                Err(BurnRateError::InvalidSampleWindow { .. })
            ));
        }
    }

    #[test]
    fn burn_rate_serde_roundtrip() {
        let rate = calculate_burn_rate(&samples(&[(1, 100), (2, 106)]), 5.0);
        let json = serde_json::to_string(&rate).unwrap();
        let decoded: BurnRate = serde_json::from_str(&json).unwrap();
        assert_eq!(rate, decoded);
    }
}
