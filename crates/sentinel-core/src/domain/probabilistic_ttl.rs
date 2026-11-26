//! Wall-clock expiration estimates based on measured ledger close intervals.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VelocityModel {
    Normal,
    LogNormal,
}

/// Measured moments of the time between consecutive ledger closes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VelocityDistribution {
    observed_at_unix_seconds: f64,
    mean_seconds: f64,
    variance_seconds_squared: f64,
    model: VelocityModel,
}

impl VelocityDistribution {
    /// Returns `None` for non-finite inputs, a nonpositive mean, or negative variance.
    pub fn new(
        observed_at_unix_seconds: f64,
        mean_seconds: f64,
        variance_seconds_squared: f64,
        model: VelocityModel,
    ) -> Option<Self> {
        if !observed_at_unix_seconds.is_finite()
            || !mean_seconds.is_finite()
            || mean_seconds <= 0.0
            || !variance_seconds_squared.is_finite()
            || variance_seconds_squared < 0.0
        {
            return None;
        }
        Some(Self {
            observed_at_unix_seconds,
            mean_seconds,
            variance_seconds_squared,
            model,
        })
    }
}

/// Unix timestamps in seconds. The confidence label is the modeled probability
/// that the entry lasts at least until that timestamp.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LifetimePercentiles {
    pub p50: f64,
    pub p90: f64,
    pub p99: f64,
}

/// Model independent close intervals. The normal model uses the sum's mean and
/// variance and floors negative durations at zero. The log-normal model fits
/// those same moments to a positive distribution (Fenton–Wilkinson).
pub fn estimate_probabilistic_lifetime(
    remaining_ledgers: u32,
    velocity_stats: &VelocityDistribution,
) -> LifetimePercentiles {
    let count = f64::from(remaining_ledgers);
    let mean = count * velocity_stats.mean_seconds;
    let deviation = count.sqrt() * velocity_stats.variance_seconds_squared.sqrt();
    let log_variance = if remaining_ledgers > 0 && velocity_stats.model == VelocityModel::LogNormal
    {
        (deviation / mean).powi(2).ln_1p()
    } else {
        0.0
    };

    let expiration = |z: f64| {
        let duration = match velocity_stats.model {
            VelocityModel::Normal => (mean + z * deviation).max(0.0),
            VelocityModel::LogNormal => mean * (z * log_variance.sqrt() - log_variance / 2.0).exp(),
        };
        velocity_stats.observed_at_unix_seconds + duration
    };

    // Inverse standard normal CDF at 0.50, 0.10 and 0.01 respectively.
    LifetimePercentiles {
        p50: expiration(0.0),
        p90: expiration(-1.281_551_565_544_600_4),
        p99: expiration(-2.326_347_874_040_840_8),
    }
}
