//! Dynamic ledger close-interval estimation based on consensus round metrics.
//!
//! The [`ConsensusVelocityEstimator`] watches consecutive ledger header
//! timestamps reported by Soroban RPC and maintains an Exponential Moving
//! Average (EMA) of the observed ledger close intervals. Because EMA weights
//! recent intervals more heavily, the estimate adapts as network consensus
//! speed changes. When the RPC history is truncated or contains too few usable
//! samples, a safe fallback of 5.0 seconds is returned.

use serde::{Deserialize, Serialize};

/// Default number of latest ledger headers sampled per estimation window.
pub const DEFAULT_SAMPLE_SIZE: usize = 50;

/// Default EMA smoothing factor: `2 / (sample_size + 1)`.
pub const DEFAULT_ALPHA: f64 = 2.0 / (DEFAULT_SAMPLE_SIZE as f64 + 1.0);

/// Fallback ledger close interval in seconds when RPC history is unavailable.
pub const FALLBACK_VELOCITY_SECONDS: f64 = 5.0;

/// Intervals below this many seconds are treated as invalid outliers.
pub const MIN_LEDGER_INTERVAL_SECONDS: f64 = 2.0;

/// Intervals above this many seconds are treated as invalid outliers.
pub const MAX_LEDGER_INTERVAL_SECONDS: f64 = 15.0;

/// Errors produced by the consensus velocity estimator.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum EstimationError {
    #[error("EMA alpha must be in the range (0.0, 1.0], got {given}")]
    InvalidAlpha { given: f64 },
    #[error("interval bounds must satisfy 0.0 < min <= max, got min={min}, max={max}")]
    InvalidBounds { min: f64, max: f64 },
}

/// One consensus round observation: a ledger sequence and its close time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ConsensusRoundSample {
    /// Ledger sequence number.
    pub sequence: u32,
    /// Ledger close time as Unix epoch seconds.
    pub close_time: i64,
}

impl ConsensusRoundSample {
    /// Create a new consensus round sample.
    pub fn new(sequence: u32, close_time: i64) -> Self {
        Self { sequence, close_time }
    }
}

/// Exponential Moving Average estimator of the ledger close interval in seconds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConsensusVelocityEstimator {
    /// EMA smoothing factor in `(0.0, 1.0]`. Higher values weight the newest
    /// interval more strongly.
    alpha: f64,
    /// Maximum number of latest ledger headers kept in the estimation window.
    sample_size: usize,
    /// Seconds-per-ledger value returned when history is insufficient.
    fallback_seconds: f64,
    /// Inclusive minimum plausible close interval (seconds).
    min_interval_seconds: f64,
    /// Inclusive maximum plausible close interval (seconds).
    max_interval_seconds: f64,
}

impl Default for ConsensusVelocityEstimator {
    fn default() -> Self {
        Self::new()
    }
}

impl ConsensusVelocityEstimator {
    /// Creates an estimator with default sampling (50 headers) and a
    /// 5.0-second fallback.
    pub fn new() -> Self {
        Self {
            alpha: DEFAULT_ALPHA,
            sample_size: DEFAULT_SAMPLE_SIZE,
            fallback_seconds: FALLBACK_VELOCITY_SECONDS,
            min_interval_seconds: MIN_LEDGER_INTERVAL_SECONDS,
            max_interval_seconds: MAX_LEDGER_INTERVAL_SECONDS,
        }
    }

    /// Sets the EMA smoothing factor. Must be in `(0.0, 1.0]`.
    pub fn with_alpha(mut self, alpha: f64) -> Result<Self, EstimationError> {
        if !(0.0 < alpha && alpha <= 1.0) {
            return Err(EstimationError::InvalidAlpha { given: alpha });
        }
        self.alpha = alpha;
        Ok(self)
    }

    /// Sets the maximum number of latest ledger headers used per window.
    pub fn with_sample_size(mut self, sample_size: usize) -> Self {
        self.sample_size = sample_size;
        self
    }

    /// Sets the fallback seconds-per-ledger returned on insufficient history.
    pub fn with_fallback(mut self, fallback_seconds: f64) -> Self {
        if fallback_seconds.is_finite() && fallback_seconds > 0.0 {
            self.fallback_seconds = fallback_seconds;
        }
        self
    }

    /// Sets the accepted interval bounds (seconds). Outliers outside
    /// `[min, max]` are rejected.
    pub fn with_bounds(mut self, min: f64, max: f64) -> Result<Self, EstimationError> {
        if !(0.0 < min && min <= max && max.is_finite() && min.is_finite()) {
            return Err(EstimationError::InvalidBounds { min, max });
        }
        self.min_interval_seconds = min;
        self.max_interval_seconds = max;
        Ok(self)
    }

    /// Estimates the average ledger close interval from consensus round samples.
    ///
    /// Samples are ordered by ledger sequence, trimmed to the latest
    /// `sample_size` headers, and consecutive close-time intervals are
    /// computed. Invalid or outlier intervals are rejected. An EMA over the
    /// remaining intervals (oldest to newest) is returned, or
    /// [`FALLBACK_VELOCITY_SECONDS`] when no usable interval exists.
    pub fn estimate(&self, samples: &[ConsensusRoundSample]) -> f64 {
        let mut ordered: Vec<ConsensusRoundSample> = samples.to_vec();
        ordered.sort_by_key(|sample| sample.sequence);

        let window = if ordered.len() > self.sample_size && self.sample_size > 0 {
            &ordered[ordered.len() - self.sample_size..]
        } else {
            &ordered
        };

        let mut ema: Option<f64> = None;
        for pair in window.windows(2) {
            let first = &pair[0];
            let second = &pair[1];

            let sequence_delta = second.sequence as i64 - first.sequence as i64;
            let time_delta = second.close_time - first.close_time;
            if sequence_delta <= 0 || time_delta <= 0 {
                continue; // non-monotonic history cannot yield an interval
            }

            let interval = time_delta as f64 / sequence_delta as f64;
            if !(self.min_interval_seconds..=self.max_interval_seconds).contains(&interval) {
                continue; // outlier / invalid interval
            }

            ema = Some(match ema {
                None => interval,
                Some(previous) => self.alpha * interval + (1.0 - self.alpha) * previous,
            });
        }

        ema.unwrap_or(self.fallback_seconds)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn steady_sequence(interval_seconds: i64, count: usize, start_sequence: u32) -> Vec<ConsensusRoundSample> {
        (0..count)
            .map(|i| {
                ConsensusRoundSample::new(
                    start_sequence + i as u32,
                    (i as i64) * interval_seconds,
                )
            })
            .collect()
    }

    #[test]
    fn test_stable_5_second_intervals() {
        let samples = steady_sequence(5, 51, 1);
        let estimate = ConsensusVelocityEstimator::new().estimate(&samples);
        assert!((estimate - 5.0).abs() < 1e-9);
    }

    #[test]
    fn test_slower_ledger_generation() {
        let samples = steady_sequence(6, 51, 1);
        let estimate = ConsensusVelocityEstimator::new().estimate(&samples);
        assert!((estimate - 6.0).abs() < 1e-9);
    }

    #[test]
    fn test_around_4_8_and_5_4_seconds() {
        // Constant intervals converge exactly regardless of sample count.
        // 4.8s per ledger: advance 5 sequences and 24 seconds per sample.
        let samples_4_8: Vec<ConsensusRoundSample> = (0..51)
            .map(|i| ConsensusRoundSample::new(1 + i * 5, i as i64 * 24))
            .collect();
        let estimate = ConsensusVelocityEstimator::new().estimate(&samples_4_8);
        assert!((estimate - 4.8).abs() < 1e-9);

        // 5.4s per ledger: advance 5 sequences and 27 seconds per sample.
        let samples_5_4: Vec<ConsensusRoundSample> = (0..51)
            .map(|i| ConsensusRoundSample::new(1 + i * 5, i as i64 * 27))
            .collect();
        let estimate = ConsensusVelocityEstimator::new().estimate(&samples_5_4);
        assert!((estimate - 5.4).abs() < 1e-9);
    }

    #[test]
    fn test_fluctuating_intervals() {
        // Alternate between 4s and 6s closes; EMA sits between, biased to recent.
        let mut samples = Vec::new();
        for i in 0..51u32 {
            let interval = if i % 2 == 0 { 4 } else { 6 };
            let close_time = samples
                .last()
                .map_or(0i64, |s: &ConsensusRoundSample| s.close_time)
                + interval;
            samples.push(ConsensusRoundSample::new(1000 + i, close_time));
        }
        let estimate = ConsensusVelocityEstimator::new().estimate(&samples);
        assert!(estimate > 4.0 && estimate < 6.0, "estimate {estimate} out of range");
    }

    #[test]
    fn test_adapts_to_changing_consensus_speed() {
        // First half of the window closes at 5s, second half at 4s. The EMA
        // must shift toward the newer, faster speed.
        let mut samples = Vec::new();
        for i in 0..60u32 {
            let interval = if i < 30 { 5 } else { 4 };
            let close_time = samples
                .last()
                .map_or(0i64, |s: &ConsensusRoundSample| s.close_time)
                + interval;
            samples.push(ConsensusRoundSample::new(i, close_time));
        }
        let estimate = ConsensusVelocityEstimator::new().estimate(&samples);
        assert!(
            estimate < 4.7,
            "expected EMA to adapt toward 4s, got {estimate}"
        );
        assert!(estimate > 4.0);
    }

    #[test]
    fn test_outlier_rejection() {
        // Intervals: 5s, 100s (outlier), 5s -> outlier rejected, estimate 5s.
        let samples = vec![
            ConsensusRoundSample::new(1, 0),
            ConsensusRoundSample::new(2, 5),
            ConsensusRoundSample::new(3, 105),
            ConsensusRoundSample::new(4, 110),
        ];
        let estimate = ConsensusVelocityEstimator::new().estimate(&samples);
        assert!((estimate - 5.0).abs() < 1e-9);
    }

    #[test]
    fn test_negative_intervals_are_skipped() {
        // Close time moving backwards must not produce a negative interval.
        let samples = vec![
            ConsensusRoundSample::new(1, 100),
            ConsensusRoundSample::new(2, 50), // invalid: time goes backward
            ConsensusRoundSample::new(3, 55),
        ];
        let estimate = ConsensusVelocityEstimator::new().estimate(&samples);
        assert!((estimate - 5.0).abs() < 1e-9);
    }

    #[test]
    fn test_insufficient_history_uses_fallback() {
        let estimator = ConsensusVelocityEstimator::new();
        assert_eq!(estimator.estimate(&[]), FALLBACK_VELOCITY_SECONDS);
        assert_eq!(
            estimator.estimate(&[ConsensusRoundSample::new(1, 1000)]),
            FALLBACK_VELOCITY_SECONDS
        );
    }

    #[test]
    fn test_all_outliers_uses_fallback() {
        let samples = vec![
            ConsensusRoundSample::new(1, 0),
            ConsensusRoundSample::new(2, 1),  // 1s interval -> below 2.0s floor
            ConsensusRoundSample::new(3, 21), // 20s interval -> above 15.0s ceiling
        ];
        let estimate = ConsensusVelocityEstimator::new().estimate(&samples);
        assert_eq!(estimate, FALLBACK_VELOCITY_SECONDS);
    }

    #[test]
    fn test_sample_window_is_truncated_to_sample_size() {
        // 101 samples: the first 51 close at 10s intervals, the last 50 at 4s
        // intervals. With a 50-header window only the 4s tail contributes.
        let mut samples = Vec::new();
        for i in 0..101u32 {
            let interval = if i < 50 { 10 } else { 4 };
            let close_time = samples
                .last()
                .map_or(0i64, |s: &ConsensusRoundSample| s.close_time)
                + interval;
            samples.push(ConsensusRoundSample::new(i, close_time));
        }
        let estimate = ConsensusVelocityEstimator::new().estimate(&samples);
        assert!((estimate - 4.0).abs() < 1e-9);
    }

    #[test]
    fn test_custom_alpha_weights_recent_intervals() {
        // Use intervals within the default [2.0, 15.0] bounds so they are not rejected as outliers.
        // Sequence 1->2 has interval 4s, sequence 2->3 has interval 14s.
        let samples = vec![
            ConsensusRoundSample::new(1, 0),
            ConsensusRoundSample::new(2, 4),
            ConsensusRoundSample::new(3, 18),
        ];
        let slow_alpha = ConsensusVelocityEstimator::new()
            .with_alpha(0.1)
            .unwrap()
            .estimate(&samples);
        let fast_alpha = ConsensusVelocityEstimator::new()
            .with_alpha(1.0)
            .unwrap()
            .estimate(&samples);
        // alpha == 1.0 reports only the latest interval (14s).
        assert!((fast_alpha - 14.0).abs() < 1e-9);
        // Smaller alpha smooths toward the older 4s interval: 0.1*14 + 0.9*4 = 5.0.
        assert!((slow_alpha - 5.0).abs() < 1e-9);
        assert!(slow_alpha < fast_alpha);
    }

    #[test]
    fn test_custom_fallback_and_bounds() {
        let estimator = ConsensusVelocityEstimator::new()
            .with_fallback(7.5)
            .with_bounds(3.0, 7.0)
            .unwrap();
        // Every interval is an outlier against the 3.0-7.0s bounds.
        let samples = vec![
            ConsensusRoundSample::new(1, 0),
            ConsensusRoundSample::new(2, 100),
        ];
        assert_eq!(estimator.estimate(&samples), 7.5);
    }

    #[test]
    fn test_invalid_alpha_is_rejected() {
        let err = ConsensusVelocityEstimator::new().with_alpha(0.0).unwrap_err();
        assert!(matches!(err, EstimationError::InvalidAlpha { .. }));
        let err = ConsensusVelocityEstimator::new().with_alpha(1.5).unwrap_err();
        assert!(matches!(err, EstimationError::InvalidAlpha { .. }));
    }

    #[test]
    fn test_invalid_bounds_are_rejected() {
        let err = ConsensusVelocityEstimator::new().with_bounds(10.0, 1.0).unwrap_err();
        assert!(matches!(err, EstimationError::InvalidBounds { .. }));
    }
}
