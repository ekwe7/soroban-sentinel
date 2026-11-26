use sentinel_core::domain::predict_initial_footprint;
use stellar_xdr::curr::ScVal;

#[test]
fn predicts_code_and_empty_instance_baseline() {
    let estimate = predict_initial_footprint(1_024, &[]);
    assert_eq!(estimate.contract_code_bytes, 1_072);
    assert_eq!(estimate.contract_instance_bytes, 128);
    assert_eq!(estimate.total_footprint_bytes, 1_200);
    assert_eq!(estimate.rent_30_day_stroops, 607_500);
    assert_eq!(estimate.rent_90_day_stroops, 1_822_500);
    assert!((estimate.rent_30_day_xlm - 0.06075).abs() < f64::EPSILON);
    assert!((estimate.rent_90_day_xlm - 0.18225).abs() < f64::EPSILON);
}

#[test]
fn constructor_values_increase_instance_footprint_and_rent() {
    let baseline = predict_initial_footprint(512, &[]);
    let initialized = predict_initial_footprint(512, &[ScVal::U32(7), ScVal::Bool(true)]);

    assert!(initialized.contract_instance_bytes > baseline.contract_instance_bytes);
    assert!(initialized.total_footprint_bytes > baseline.total_footprint_bytes);
    assert!(initialized.rent_30_day_stroops > baseline.rent_30_day_stroops);
    assert!(initialized.rent_90_day_stroops > baseline.rent_90_day_stroops);
}

#[test]
fn rent_reserve_scales_linearly_with_duration() {
    let estimate = predict_initial_footprint(0, &[]);
    assert_eq!(
        estimate.rent_90_day_stroops,
        estimate.rent_30_day_stroops * 3
    );
}
