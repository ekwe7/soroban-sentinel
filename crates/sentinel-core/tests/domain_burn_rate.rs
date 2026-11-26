use sentinel_core::domain::{
    calculate_burn_rate, project_exhaustion, validate_sample_window, BurnRate, BurnRateError,
    BurnRateSource, LedgerCloseSample, DEFAULT_SAMPLE_WINDOW_LEDGERS, MAX_SAMPLE_WINDOW_LEDGERS,
    TARGET_LEDGER_CLOSE_SECONDS,
};

fn history(pairs: &[(u32, i64)]) -> Vec<LedgerCloseSample> {
    pairs
        .iter()
        .map(|(sequence, close_time)| LedgerCloseSample::new(*sequence, *close_time))
        .collect()
}

#[test]
fn calculates_velocity_across_irregular_close_intervals() {
    let samples = history(&[
        (5_400_000, 1_700_000_000),
        (5_400_001, 1_700_000_004),
        (5_400_002, 1_700_000_011),
        (5_400_003, 1_700_000_014),
        (5_400_004, 1_700_000_020),
    ]);

    let rate = calculate_burn_rate(&samples, TARGET_LEDGER_CLOSE_SECONDS);

    assert_eq!(rate.source, BurnRateSource::Measured);
    assert!((rate.seconds_per_ledger - 5.0).abs() < 1e-9);
    assert!((rate.ledgers_per_second - 0.2).abs() < 1e-9);
    assert_eq!(rate.sample_count, 5);
    assert_eq!(rate.observed_ledgers, 4);
    assert_eq!(rate.latest_close_time, Some(1_700_000_020));
}

#[test]
fn projects_exhaustion_for_remaining_ledgers() {
    let samples = history(&[(1, 1_000), (2, 1_006), (3, 1_012)]);
    let rate = calculate_burn_rate(&samples, TARGET_LEDGER_CLOSE_SECONDS);
    assert!((rate.seconds_per_ledger - 6.0).abs() < 1e-9);

    let projection = project_exhaustion(&rate, 10_000, rate.latest_close_time).unwrap();
    assert_eq!(projection.remaining_ledgers, 10_000);
    assert!((projection.remaining_seconds - 60_000.0).abs() < 1e-9);
    assert_eq!(projection.exhaustion_timestamp, Some(1_012 + 60_000));
}

#[test]
fn unavailable_history_uses_five_second_fallback() {
    let rate = calculate_burn_rate(&[], TARGET_LEDGER_CLOSE_SECONDS);

    assert_eq!(rate.source, BurnRateSource::Fallback);
    assert_eq!(rate.seconds_per_ledger, 5.0);
    assert!(!rate.is_measured());
}

#[test]
fn invalid_intervals_do_not_produce_negative_or_zero_velocity() {
    let zero_interval = history(&[(100, 1_000), (101, 1_000), (102, 1_000)]);
    let negative_interval = history(&[(100, 1_000), (101, 990), (102, 980)]);
    let invalid_time = history(&[(100, 0), (101, -1), (102, 1_000)]);

    for samples in [zero_interval, negative_interval, invalid_time] {
        let rate = calculate_burn_rate(&samples, TARGET_LEDGER_CLOSE_SECONDS);
        assert_eq!(rate.source, BurnRateSource::Fallback);
        assert_eq!(rate.seconds_per_ledger, TARGET_LEDGER_CLOSE_SECONDS);
        assert!(rate.ledgers_per_second > 0.0);
    }
}

#[test]
fn duplicate_and_unsorted_samples_are_normalized() {
    let samples = history(&[
        (102, 1_010),
        (100, 1_000),
        (101, 1_005),
        (101, 1_006),
        (103, 1_015),
    ]);

    let rate = calculate_burn_rate(&samples, TARGET_LEDGER_CLOSE_SECONDS);

    assert_eq!(rate.source, BurnRateSource::Measured);
    assert_eq!(rate.sample_count, 4);
    assert_eq!(rate.observed_ledgers, 3);
    assert!((rate.seconds_per_ledger - 5.0).abs() < 1e-9);
}

#[test]
fn sample_window_validation_and_defaults() {
    assert_eq!(
        validate_sample_window(DEFAULT_SAMPLE_WINDOW_LEDGERS).unwrap(),
        DEFAULT_SAMPLE_WINDOW_LEDGERS
    );
    assert_eq!(
        validate_sample_window(MAX_SAMPLE_WINDOW_LEDGERS).unwrap(),
        MAX_SAMPLE_WINDOW_LEDGERS
    );
    assert!(matches!(
        validate_sample_window(0),
        Err(BurnRateError::InvalidSampleWindow { .. })
    ));
    assert!(matches!(
        validate_sample_window(MAX_SAMPLE_WINDOW_LEDGERS + 1),
        Err(BurnRateError::InvalidSampleWindow { .. })
    ));
}

#[test]
fn measured_velocity_outside_plausible_range_falls_back() {
    for seconds in [0.25, 90.0, f64::NAN, f64::INFINITY] {
        let rate = BurnRate::measured(seconds);
        assert_eq!(rate.source, BurnRateSource::Fallback);
        assert_eq!(rate.seconds_per_ledger, TARGET_LEDGER_CLOSE_SECONDS);
    }
}
