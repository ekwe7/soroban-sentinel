//! Congestion-aware inclusion fee recommendations with an explicit spending cap.

use thiserror::Error;

/// Network-reported Soroban inclusion fee percentiles, in stroops.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FeePercentiles {
    pub p50: u64,
    pub p80: u64,
    pub p90: u64,
}

/// Urgency level used to select a fee percentile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeeUrgency {
    Normal,
    Elevated,
    Critical,
}

/// Capped fee recommendation and the percentile used to calculate it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FeeRecommendation {
    pub percentile: u8,
    pub fee_stroops: u64,
    pub capped: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum FeeBumpError {
    #[error("maximum inclusion fee must be greater than zero")]
    InvalidFeeCap,
    #[error("network fee percentile must be greater than zero")]
    InvalidPercentileFee,
    #[error("network fee percentiles must be nondecreasing")]
    InvalidPercentileOrder,
}

/// Selects p50 for normal, p80 for elevated, and p90 for critical urgency.
/// The returned fee never exceeds `max_fee_stroops`.
pub fn recommend_inclusion_fee(
    percentiles: FeePercentiles,
    urgency: FeeUrgency,
    max_fee_stroops: u64,
) -> Result<FeeRecommendation, FeeBumpError> {
    if max_fee_stroops == 0 {
        return Err(FeeBumpError::InvalidFeeCap);
    }
    if percentiles.p50 > percentiles.p80 || percentiles.p80 > percentiles.p90 {
        return Err(FeeBumpError::InvalidPercentileOrder);
    }

    let (percentile, observed_fee) = match urgency {
        FeeUrgency::Normal => (50, percentiles.p50),
        FeeUrgency::Elevated => (80, percentiles.p80),
        FeeUrgency::Critical => (90, percentiles.p90),
    };
    if observed_fee == 0 {
        return Err(FeeBumpError::InvalidPercentileFee);
    }

    Ok(FeeRecommendation {
        percentile,
        fee_stroops: observed_fee.min(max_fee_stroops),
        capped: observed_fee > max_fee_stroops,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn percentiles() -> FeePercentiles {
        FeePercentiles {
            p50: 100,
            p80: 500,
            p90: 900,
        }
    }

    #[test]
    fn urgency_selects_expected_network_percentile() {
        assert_eq!(
            recommend_inclusion_fee(percentiles(), FeeUrgency::Normal, 1_000).unwrap(),
            FeeRecommendation {
                percentile: 50,
                fee_stroops: 100,
                capped: false,
            }
        );
        assert_eq!(
            recommend_inclusion_fee(percentiles(), FeeUrgency::Elevated, 1_000)
                .unwrap()
                .fee_stroops,
            500
        );
        assert_eq!(
            recommend_inclusion_fee(percentiles(), FeeUrgency::Critical, 1_000).unwrap(),
            FeeRecommendation {
                percentile: 90,
                fee_stroops: 900,
                capped: false,
            }
        );
    }

    #[test]
    fn applies_maximum_fee_safety_cap() {
        let recommendation =
            recommend_inclusion_fee(percentiles(), FeeUrgency::Critical, 600).unwrap();
        assert_eq!(recommendation.fee_stroops, 600);
        assert!(recommendation.capped);
    }

    #[test]
    fn rejects_zero_fee_cap_and_missing_percentile_fee() {
        assert_eq!(
            recommend_inclusion_fee(percentiles(), FeeUrgency::Critical, 0),
            Err(FeeBumpError::InvalidFeeCap)
        );
        assert_eq!(
            recommend_inclusion_fee(
                FeePercentiles {
                    p50: 0,
                    p80: 0,
                    p90: 0,
                },
                FeeUrgency::Critical,
                10
            ),
            Err(FeeBumpError::InvalidPercentileFee)
        );
        assert_eq!(
            recommend_inclusion_fee(
                FeePercentiles {
                    p50: 100,
                    p80: 90,
                    p90: 200,
                },
                FeeUrgency::Critical,
                300
            ),
            Err(FeeBumpError::InvalidPercentileOrder)
        );
    }
}
