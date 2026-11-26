use sentinel_core::domain::half_life::{
    fixed_point_exp_neg, RetentionMode, StorageChurnModel, FIXED_POINT_SCALE, LN_2_SCALED,
};

#[test]
fn test_half_life_formula_and_calculation() {
    // Test t_1/2 = ln(2) / lambda for lambda = 0.05
    let model = StorageChurnModel::from_scaled_lambda(50_000);
    assert_eq!(model.mode, RetentionMode::ActiveChurn);
    assert_eq!(model.lambda_scaled, 50_000);

    // ln(2) / 0.05 ≈ 13.86 ledgers -> integer ledgers = 13
    let half_life = model.half_life_ledgers().expect("half life should exist");
    assert_eq!(half_life, 13);

    let half_life_scaled = model
        .half_life_scaled()
        .expect("scaled half life should exist");
    // (693_147 * 1_000_000) / 50_000 = 13_862_940 (13.862940 ledgers)
    assert_eq!(half_life_scaled, 13_862_940);
}

#[test]
fn test_model_from_observations() {
    // 500 mutations observed over 10_000 ledgers -> lambda = 0.05 (50_000 scaled)
    let model = StorageChurnModel::from_observations(RetentionMode::ActiveChurn, 500, 10_000);
    assert_eq!(model.lambda_scaled, 50_000);
    assert_eq!(model.half_life_ledgers(), Some(13));
}

#[test]
fn test_static_retention_mode() {
    let model = StorageChurnModel::static_mode();
    assert_eq!(model.mode, RetentionMode::Static);
    assert_eq!(model.half_life_ledgers(), None);

    let initial_volume = 10_000;
    let initial_ttl = 1_000;
    let curve = model.depletion_curve(initial_volume, initial_ttl, 1_000, 250);

    assert_eq!(curve.len(), 5);
    // At t=0: 10_000
    assert_eq!(curve[0].remaining_volume, 10_000);
    assert_eq!(curve[0].depletion_percent, 0);

    // At t=250: 7_500 (25% depleted)
    assert_eq!(curve[1].remaining_volume, 7_500);
    assert_eq!(curve[1].depletion_percent, 25);

    // At t=500: 5_000 (50% depleted)
    assert_eq!(curve[2].remaining_volume, 5_000);
    assert_eq!(curve[2].depletion_percent, 50);

    // At t=750: 2_500 (75% depleted)
    assert_eq!(curve[3].remaining_volume, 2_500);
    assert_eq!(curve[3].depletion_percent, 75);

    // At t=1000: 0 (100% depleted)
    assert_eq!(curve[4].remaining_volume, 0);
    assert_eq!(curve[4].depletion_percent, 100);
}

#[test]
fn test_active_churn_retention_mode() {
    let model = StorageChurnModel::from_scaled_lambda(50_000);
    let initial_volume = 10_000;
    let initial_ttl = 1_000;
    let curve = model.depletion_curve(initial_volume, initial_ttl, 100, 25);

    assert_eq!(curve[0].remaining_volume, 10_000);
    // Depletion strictly increases monotonically
    for i in 1..curve.len() {
        assert!(curve[i].remaining_volume <= curve[i - 1].remaining_volume);
        assert!(curve[i].depletion_percent >= curve[i - 1].depletion_percent);
    }
}

#[test]
fn test_mathematical_convergence_across_diverse_churn_rates() {
    // Acceptance criterion:
    // Unit tests verify mathematical convergence across diverse entry churn rates (0.01 <= lambda <= 0.5)

    let test_lambdas_scaled = [
        10_000, 20_000, 50_000, 75_000, 100_000, 150_000, 200_000, 250_000, 300_000, 400_000,
        500_000,
    ];

    for &lambda_scaled in &test_lambdas_scaled {
        let model = StorageChurnModel::from_scaled_lambda(lambda_scaled);
        assert!(
            (10_000..=500_000).contains(&lambda_scaled),
            "lambda_scaled {} out of expected range for lambda {}",
            lambda_scaled,
            lambda_scaled
        );

        let half_life_scaled = model.half_life_scaled().expect("half life must exist");
        assert!(
            half_life_scaled > 0,
            "half life must be positive for lambda {}",
            lambda_scaled
        );

        // At exactly 1 half-life (t = t_1/2), expected retention fraction is 0.5 (500_000 / 1_000_000).
        // Let's test fixed_point_exp_neg at x = lambda * t_1/2 = ln(2):
        let x_at_half_life =
            (lambda_scaled as u128 * half_life_scaled as u128 / FIXED_POINT_SCALE as u128) as u64;
        // x_at_half_life should be extremely close to LN_2_SCALED
        assert!(
            (x_at_half_life as i64 - LN_2_SCALED as i64).abs() <= 2,
            "x at half life mismatch for lambda {}: got {}, expected {}",
            lambda_scaled,
            x_at_half_life,
            LN_2_SCALED
        );

        let retention_at_half_life = fixed_point_exp_neg(x_at_half_life);
        // Should be 500_000 ± 5 (0.500000 ± 0.000005)
        assert!(
            (retention_at_half_life as i64 - 500_000).abs() <= 5,
            "Retention at half-life did not converge to 0.5 for lambda {}: got {}",
            lambda_scaled,
            retention_at_half_life
        );

        // At 2 half-lives (t = 2 * t_1/2), expected retention fraction is 0.25 (250_000 / 1_000_000).
        let retention_at_two_half_lives = fixed_point_exp_neg(x_at_half_life * 2);
        assert!(
            (retention_at_two_half_lives as i64 - 250_000).abs() <= 5,
            "Retention at 2 half-lives did not converge to 0.25 for lambda {}: got {}",
            lambda_scaled,
            retention_at_two_half_lives
        );

        // At 3 half-lives (t = 3 * t_1/2), expected retention fraction is 0.125 (125_000 / 1_000_000).
        let retention_at_three_half_lives = fixed_point_exp_neg(x_at_half_life * 3);
        assert!(
            (retention_at_three_half_lives as i64 - 125_000).abs() <= 5,
            "Retention at 3 half-lives did not converge to 0.125 for lambda {}: got {}",
            lambda_scaled,
            retention_at_three_half_lives
        );

        // Monotonic decay verification over a sequence of 20 steps
        let curve = model.depletion_curve(1_000_000, 10_000, 200, 10);
        for step in 1..curve.len() {
            assert!(
                curve[step].remaining_volume <= curve[step - 1].remaining_volume,
                "Depletion curve must monotonically decrease for lambda {}",
                lambda_scaled
            );
            assert!(
                curve[step].retention_ratio_scaled <= curve[step - 1].retention_ratio_scaled,
                "Retention ratio must monotonically decrease for lambda {}",
                lambda_scaled
            );
        }
    }
}
