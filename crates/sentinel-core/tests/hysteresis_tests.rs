use sentinel_core::{AlertAction, HysteresisConfig, HysteresisFilter, RiskLevel, ThresholdConfig};

fn filter(warning_breach_scans: usize, stable_scans: usize) -> HysteresisFilter {
    HysteresisFilter::with_config(HysteresisConfig {
        thresholds: ThresholdConfig::new(10, 3),
        warning_breach_scans,
        stable_scans,
    })
}

#[test]
fn default_filter_requires_two_warning_breaches_and_two_stable_scans() {
    let mut filter = HysteresisFilter::new();
    let warning = 100_000;
    let healthy = 120_960;

    assert_eq!(filter.update("instance", warning), AlertAction::Suppress);
    assert_eq!(filter.active_risk("instance"), Some(RiskLevel::Ok));
    assert_eq!(filter.update("instance", healthy), AlertAction::Suppress);
    assert_eq!(filter.update("instance", warning), AlertAction::Suppress);
    assert_eq!(filter.update("instance", warning), AlertAction::FireAlert);
    assert_eq!(filter.active_risk("instance"), Some(RiskLevel::Warn));
    assert_eq!(filter.update("instance", warning), AlertAction::Suppress);

    assert_eq!(filter.update("instance", healthy), AlertAction::Suppress);
    assert_eq!(filter.update("instance", warning), AlertAction::Suppress);
    assert_eq!(filter.update("instance", healthy), AlertAction::Suppress);
    assert_eq!(filter.update("instance", healthy), AlertAction::Clear);
    assert_eq!(filter.active_risk("instance"), Some(RiskLevel::Ok));
    assert_eq!(filter.update("instance", healthy), AlertAction::Suppress);
}

#[test]
fn custom_transition_counts_are_applied_per_key() {
    let mut filter = filter(3, 2);

    assert_eq!(filter.update("key-a", 9), AlertAction::Suppress);
    assert_eq!(filter.update("key-a", 9), AlertAction::Suppress);
    assert_eq!(filter.update("key-b", 9), AlertAction::Suppress);
    assert_eq!(filter.update("key-a", 9), AlertAction::FireAlert);
    assert_eq!(filter.active_risk("key-a"), Some(RiskLevel::Warn));
    assert_eq!(filter.active_risk("key-b"), Some(RiskLevel::Ok));

    assert_eq!(filter.update("key-a", 10), AlertAction::Suppress);
    assert_eq!(filter.update("key-a", 10), AlertAction::Clear);
}

#[test]
fn critical_threshold_bypasses_warning_hysteresis_immediately() {
    let mut filter = filter(4, 2);

    assert_eq!(filter.update("key", 9), AlertAction::Suppress);
    assert_eq!(filter.update("key", 2), AlertAction::FireAlert);
    assert_eq!(filter.active_risk("key"), Some(RiskLevel::Critical));
    assert_eq!(filter.update("key", 2), AlertAction::Suppress);
}

#[test]
fn archived_risk_fires_immediately_and_severity_escalation_is_not_delayed() {
    let mut filter = filter(2, 2);

    assert_eq!(filter.update("key", 9), AlertAction::Suppress);
    assert_eq!(filter.update("key", 9), AlertAction::FireAlert);
    assert_eq!(filter.update("key", 2), AlertAction::FireAlert);
    assert_eq!(filter.active_risk("key"), Some(RiskLevel::Critical));
    assert_eq!(filter.update("key", 0), AlertAction::FireAlert);
    assert_eq!(filter.active_risk("key"), Some(RiskLevel::Archived));
}

#[test]
fn warning_and_critical_scans_reset_consecutive_recovery_progress() {
    let mut filter = filter(1, 3);

    assert_eq!(filter.update("key", 9), AlertAction::FireAlert);
    assert_eq!(filter.update("key", 10), AlertAction::Suppress);
    assert_eq!(filter.update("key", 9), AlertAction::Suppress);
    assert_eq!(filter.update("key", 10), AlertAction::Suppress);
    assert_eq!(filter.update("key", 2), AlertAction::FireAlert);
    assert_eq!(filter.update("key", 10), AlertAction::Suppress);
    assert_eq!(filter.update("key", 10), AlertAction::Suppress);
    assert_eq!(filter.update("key", 10), AlertAction::Clear);
    assert_eq!(filter.active_risk("key"), Some(RiskLevel::Ok));
}
