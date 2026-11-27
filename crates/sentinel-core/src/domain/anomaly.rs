use std::collections::VecDeque;

/// Default number of ledger-velocity observations used for anomaly detection.
pub const DEFAULT_ANOMALY_WINDOW_SIZE: usize = 30;

/// Maximum absolute z-score accepted as an ordinary observation.
pub const ANOMALY_Z_SCORE_THRESHOLD: f64 = 3.0;

/// Filters isolated ledger-velocity outliers using a rolling statistical window.
#[derive(Debug, Clone)]
pub struct LedgerAnomalyFilter {
    window: VecDeque<f64>,
    window_size: usize,
}

impl Default for LedgerAnomalyFilter {
    fn default() -> Self {
        Self::new(DEFAULT_ANOMALY_WINDOW_SIZE)
    }
}

impl LedgerAnomalyFilter {
    /// Creates a filter with a fixed maximum window size.
    ///
    /// A zero-sized window is treated as a one-observation window.
    pub fn new(window_size: usize) -> Self {
        Self {
            window: VecDeque::with_capacity(window_size.max(1)),
            window_size: window_size.max(1),
        }
    }

    /// Returns the sample unless it is a statistical outlier, in which case
    /// the historical median is returned.
    pub fn observe(&mut self, sample: f64) -> f64 {
        if !sample.is_finite() || sample <= 0.0 {
            return self.historical_median().unwrap_or(5.0);
        }

        let fallback = self.historical_median();
        let is_outlier = if self.window.is_empty() {
            false
        } else {
            let mean = self.window.iter().sum::<f64>() / self.window.len() as f64;
            let variance = self
                .window
                .iter()
                .map(|value| (value - mean).powi(2))
                .sum::<f64>()
                / self.window.len() as f64;
            let standard_deviation = variance.sqrt();

            if standard_deviation == 0.0 {
                sample != mean
            } else {
                ((sample - mean) / standard_deviation).abs() > ANOMALY_Z_SCORE_THRESHOLD
            }
        };

        let filtered = if is_outlier {
            fallback.unwrap_or(sample)
        } else {
            sample
        };
        // Keep observations in the window so a sustained velocity shift can settle.
        self.push(sample);
        filtered
    }

    fn push(&mut self, sample: f64) {
        if self.window.len() == self.window_size {
            self.window.pop_front();
        }
        self.window.push_back(sample);
    }

    fn historical_median(&self) -> Option<f64> {
        if self.window.is_empty() {
            return None;
        }

        let mut values: Vec<_> = self.window.iter().copied().collect();
        values.sort_by(f64::total_cmp);
        let middle = values.len() / 2;
        Some(if values.len() % 2 == 0 {
            (values[middle - 1] + values[middle]) / 2.0
        } else {
            values[middle]
        })
    }
}
