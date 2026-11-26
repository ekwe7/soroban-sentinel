//! Storage entry half-life decay and dynamic churn velocity modeling.
//!
//! Assuming linear ledger decay (1.0 ledger remaining per ledger elapsed) is accurate
//! only for static entries. In real-world smart contracts, storage entries experience
//! variable churn (rapid overwrites, balance updates, transient key expansion).
//! A half-life decay model calculates non-linear decay velocity based on write frequency,
//! giving operators an empirical forecast of storage longevity under active transaction churn.

use serde::{Deserialize, Serialize};

/// Fixed-point scaling factor (10^6).
pub const FIXED_POINT_SCALE: u64 = 1_000_000;

/// Scaled natural log of 2: ln(2) * 1_000_000 ≈ 693_147.
pub const LN_2_SCALED: u64 = 693_147;

/// Lookup table for e^(-n) * FIXED_POINT_SCALE for integer n in 0..=14.
const EXP_NEG_INTEGER_TABLE: [u64; 15] = [
    1_000_000, // e^0
    367_879,   // e^-1
    135_335,   // e^-2
    49_787,    // e^-3
    18_316,    // e^-4
    6_738,     // e^-5
    2_479,     // e^-6
    912,       // e^-7
    335,       // e^-8
    123,       // e^-9
    45,        // e^-10
    17,        // e^-11
    6,         // e^-12
    2,         // e^-13
    1,         // e^-14
];

/// Mode of storage entry retention evaluation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetentionMode {
    /// Linear ledger decay: 1 ledger elapsed = 1 ledger remaining deducted.
    Static,
    /// Non-linear half-life decay modeling active transaction churn and mutation velocity.
    ActiveChurn,
}

impl std::fmt::Display for RetentionMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Static => write!(f, "static"),
            Self::ActiveChurn => write!(f, "active_churn"),
        }
    }
}

/// A single step in an expected storage depletion curve.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DepletionStep {
    /// Ledger offset from evaluation starting point.
    pub ledger_offset: u32,
    /// Remaining storage units or entries estimated at this step.
    pub remaining_volume: u64,
    /// Remaining retention fraction scaled by [`FIXED_POINT_SCALE`] (0 to 1_000_000).
    pub retention_ratio_scaled: u64,
    /// Depletion percentage (0 to 100).
    pub depletion_percent: u32,
}

/// Dynamic storage churn model calculating effective half-life decay and depletion curves.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StorageChurnModel {
    /// Retention mode applied during calculations.
    pub mode: RetentionMode,
    /// Scaled decay rate (lambda * FIXED_POINT_SCALE).
    pub lambda_scaled: u64,
    /// Number of entry mutations observed.
    pub mutations: u32,
    /// Number of ledgers elapsed during observation window.
    pub ledgers_elapsed: u32,
}

impl StorageChurnModel {
    /// Construct a new model with static retention mode.
    pub fn static_mode() -> Self {
        Self {
            mode: RetentionMode::Static,
            lambda_scaled: 0,
            mutations: 0,
            ledgers_elapsed: 0,
        }
    }

    /// Construct an active churn model with explicit scaled decay rate `lambda_scaled`.
    ///
    /// For example, `lambda = 0.05` corresponds to `lambda_scaled = 50_000`.
    pub fn from_scaled_lambda(lambda_scaled: u64) -> Self {
        Self {
            mode: RetentionMode::ActiveChurn,
            lambda_scaled,
            mutations: 0,
            ledgers_elapsed: 0,
        }
    }

    /// Construct a model from observed entry mutation frequency over an elapsed ledger window.
    ///
    /// Churn rate lambda = mutations / ledgers_elapsed.
    pub fn from_observations(mode: RetentionMode, mutations: u32, ledgers_elapsed: u32) -> Self {
        let lambda_scaled = if ledgers_elapsed == 0 || mutations == 0 {
            0
        } else {
            (mutations as u64 * FIXED_POINT_SCALE) / ledgers_elapsed as u64
        };
        Self {
            mode,
            lambda_scaled,
            mutations,
            ledgers_elapsed,
        }
    }

    /// Computes effective decay half-life in integer ledgers: t_1/2 = ln(2) / lambda.
    ///
    /// Returns `None` if churn rate is zero or mode is static.
    pub fn half_life_ledgers(&self) -> Option<u64> {
        if self.mode == RetentionMode::Static || self.lambda_scaled == 0 {
            return None;
        }
        Some(LN_2_SCALED / self.lambda_scaled)
    }

    /// Computes scaled half life with sub-ledger precision (scaled by [`FIXED_POINT_SCALE`]).
    pub fn half_life_scaled(&self) -> Option<u64> {
        if self.mode == RetentionMode::Static || self.lambda_scaled == 0 {
            return None;
        }
        Some((LN_2_SCALED * FIXED_POINT_SCALE) / self.lambda_scaled)
    }

    /// Evaluates expected retention ratio scaled by [`FIXED_POINT_SCALE`] at elapsed ledger offset `t`.
    ///
    /// In static mode: linear decay `max(0, 1 - t / initial_ttl)`.
    /// In active churn mode: non-linear exponential decay `e^(-lambda * t)` using deterministic fixed-point math.
    pub fn retention_ratio_scaled(&self, ledgers_elapsed: u32, initial_ttl: u32) -> u64 {
        match self.mode {
            RetentionMode::Static => {
                if initial_ttl == 0 || ledgers_elapsed >= initial_ttl {
                    0
                } else {
                    let remaining = initial_ttl - ledgers_elapsed;
                    (remaining as u64 * FIXED_POINT_SCALE) / initial_ttl as u64
                }
            }
            RetentionMode::ActiveChurn => {
                if self.lambda_scaled == 0 {
                    return FIXED_POINT_SCALE;
                }
                let x_scaled = self.lambda_scaled.saturating_mul(ledgers_elapsed as u64);
                fixed_point_exp_neg(x_scaled)
            }
        }
    }

    /// Calculates expected storage depletion curve for an initial entry volume over `horizon_ledgers`.
    pub fn depletion_curve(
        &self,
        initial_volume: u64,
        initial_ttl: u32,
        horizon_ledgers: u32,
        step_ledgers: u32,
    ) -> Vec<DepletionStep> {
        let step_size = step_ledgers.max(1);
        let mut steps = Vec::new();
        let mut current_offset = 0u32;

        while current_offset <= horizon_ledgers {
            let ratio = self.retention_ratio_scaled(current_offset, initial_ttl);
            let remaining =
                ((initial_volume as u128 * ratio as u128) / FIXED_POINT_SCALE as u128) as u64;
            let depletion_percent = if initial_volume == 0 {
                0
            } else {
                ((initial_volume.saturating_sub(remaining) as u128 * 100) / initial_volume as u128)
                    as u32
            };

            steps.push(DepletionStep {
                ledger_offset: current_offset,
                remaining_volume: remaining,
                retention_ratio_scaled: ratio,
                depletion_percent,
            });

            if current_offset == horizon_ledgers {
                break;
            }
            current_offset = current_offset
                .saturating_add(step_size)
                .min(horizon_ledgers);
        }

        steps
    }
}

/// Evaluates e^(-x) where `x_scaled = x * FIXED_POINT_SCALE` using deterministic fixed-point math.
///
/// Returns a value scaled by [`FIXED_POINT_SCALE`] in range [0, 1_000_000].
pub fn fixed_point_exp_neg(x_scaled: u64) -> u64 {
    let integer_part = x_scaled / FIXED_POINT_SCALE;
    if integer_part >= EXP_NEG_INTEGER_TABLE.len() as u64 {
        return 0;
    }
    let integer_part = integer_part as usize;

    let exp_int = EXP_NEG_INTEGER_TABLE[integer_part];
    let f_scaled = (x_scaled % FIXED_POINT_SCALE) as u128; // fractional part in [0, 1_000_000)

    if f_scaled == 0 {
        return exp_int;
    }

    // Evaluate the alternating Taylor series with integer fixed-point terms.
    let scale = FIXED_POINT_SCALE as u128;
    let mut term = scale;
    let mut exp_frac = scale;
    for n in 1..=12u128 {
        term = (term * f_scaled) / (n * scale);
        if n % 2 == 0 {
            exp_frac += term;
        } else {
            exp_frac = exp_frac.saturating_sub(term);
        }
    }

    // Combine integer and fractional parts: e^(-x) = e^(-n) * e^(-f)
    ((exp_int as u128 * exp_frac) / scale) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exp_neg_convergence_and_accuracy() {
        // e^0 = 1.0 -> 1_000_000
        assert_eq!(fixed_point_exp_neg(0), 1_000_000);

        // e^-ln(2) = 0.5 -> 500_000
        let half = fixed_point_exp_neg(LN_2_SCALED);
        assert!((half as i64 - 500_000).abs() <= 1);

        // e^-1 ≈ 0.367879 -> 367_879
        let e1 = fixed_point_exp_neg(1_000_000);
        assert!((e1 as i64 - 367_879).abs() <= 2);
    }

    #[test]
    fn test_static_vs_active_churn_modes() {
        let initial_volume = 1000;
        let initial_ttl = 100;

        let static_model = StorageChurnModel::static_mode();
        let curve_static = static_model.depletion_curve(initial_volume, initial_ttl, 100, 50);
        assert_eq!(curve_static.len(), 3);
        assert_eq!(curve_static[0].remaining_volume, 1000);
        assert_eq!(curve_static[1].remaining_volume, 500); // at t=50, half elapsed
        assert_eq!(curve_static[2].remaining_volume, 0); // at t=100, expired

        let churn_model = StorageChurnModel::from_scaled_lambda(50_000); // lambda = 0.05
        let curve_churn = churn_model.depletion_curve(initial_volume, initial_ttl, 100, 50);
        assert_eq!(curve_churn[0].remaining_volume, 1000);
        // Exponential decay is steeper than linear
        assert!(curve_churn[1].remaining_volume < 500);
    }

    #[test]
    fn test_half_life_calculation() {
        let model = StorageChurnModel::from_scaled_lambda(50_000); // 0.05
        assert_eq!(model.half_life_ledgers(), Some(13)); // 693_147 / 50_000 = 13
        assert_eq!(model.half_life_scaled(), Some(13_862_940)); // 13.862940 ledgers
    }
}
