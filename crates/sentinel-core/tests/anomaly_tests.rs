use sentinel_core::{LedgerAnomalyFilter, DEFAULT_ANOMALY_WINDOW_SIZE};

#[test]
fn suppresses_a_single_thirty_second_velocity_spike() {
    let mut filter = LedgerAnomalyFilter::default();
    for _ in 0..DEFAULT_ANOMALY_WINDOW_SIZE {
        assert_eq!(filter.observe(5.0), 5.0);
    }

    assert_eq!(filter.observe(30.0), 5.0);
    assert_eq!(filter.observe(5.0), 5.0);
}

#[test]
fn adapts_to_a_sustained_velocity_change() {
    let mut filter = LedgerAnomalyFilter::default();
    for _ in 0..DEFAULT_ANOMALY_WINDOW_SIZE {
        filter.observe(5.0);
    }

    assert_eq!(filter.observe(8.0), 5.0);
    assert_eq!(filter.observe(8.0), 5.0);
    assert_eq!(filter.observe(8.0), 5.0);
    assert_eq!(filter.observe(8.0), 8.0);
    assert_eq!(filter.observe(8.0), 8.0);
}
