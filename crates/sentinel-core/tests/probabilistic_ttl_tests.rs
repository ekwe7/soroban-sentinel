use sentinel_core::domain::{
    estimate_probabilistic_lifetime, LifetimePercentiles, VelocityDistribution, VelocityModel,
};

#[test]
fn normal_percentiles_use_the_lower_tail_for_conservative_expiration() {
    let stats = VelocityDistribution::new(1_000.0, 5.0, 4.0, VelocityModel::Normal).unwrap();
    let lifetime = estimate_probabilistic_lifetime(100, &stats);
    assert_eq!(lifetime.p50, 1_500.0);
    assert!((lifetime.p90 - 1_474.368_968_689).abs() < 1e-8);
    assert!((lifetime.p99 - 1_453.473_042_519).abs() < 1e-8);
}

#[test]
fn log_normal_percentiles_are_ordered_and_positive() {
    let stats = VelocityDistribution::new(1_000.0, 5.0, 25.0, VelocityModel::LogNormal).unwrap();
    let lifetime = estimate_probabilistic_lifetime(1, &stats);
    assert!((lifetime.p50 - (1_000.0 + 5.0 / 2_f64.sqrt())).abs() < 1e-8);
    assert!(lifetime.p50 > lifetime.p90);
    assert!(lifetime.p90 > lifetime.p99);
    assert!(lifetime.p99 > 1_000.0);
}

#[test]
fn zero_variance_and_zero_ledgers_have_deterministic_timestamps() {
    for model in [VelocityModel::Normal, VelocityModel::LogNormal] {
        let stats = VelocityDistribution::new(1_000.0, 5.0, 0.0, model).unwrap();
        for (ledgers, timestamp) in [(0, 1_000.0), (4, 1_020.0)] {
            assert_eq!(
                estimate_probabilistic_lifetime(ledgers, &stats),
                LifetimePercentiles {
                    p50: timestamp,
                    p90: timestamp,
                    p99: timestamp,
                }
            );
        }
    }
}
