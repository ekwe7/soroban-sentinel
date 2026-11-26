//! Pure estimates for modeling proposed Soroban rent and TTL parameter changes.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::domain::{evaluate_risk, RiskLevel, ThresholdConfig};

/// Network rent and horizon parameters used by a protocol scenario.
///
/// The cost model treats `fee_rate_per_byte` as stroops per byte per rent
/// extension, and estimates monthly extensions as `ledgers_per_month /
/// minimum_persistent_ttl`. This is a comparative planning estimate, not an
/// exact transaction quote; the caller must supply the network's fee values.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct NetworkRentParameters {
    /// Rent fee in stroops per byte for one extension.
    pub fee_rate_per_byte: f64,
    /// Multiplier applied to the base rent fee.
    pub rent_write_fee_multiplier: f64,
    /// Renewal horizon in ledgers used to estimate persistent rent cadence.
    pub minimum_persistent_ttl: u32,
    /// Remaining-ledger horizon below which entries are flagged for warning.
    pub warning_ledgers: u32,
    /// Remaining-ledger horizon below which entries are flagged as critical.
    pub critical_ledgers: u32,
    /// Number of ledgers used to represent one month in upkeep estimates.
    pub ledgers_per_month: u32,
}

impl NetworkRentParameters {
    /// Constructs and validates a set of network rent and horizon parameters.
    pub fn try_new(
        fee_rate_per_byte: f64,
        rent_write_fee_multiplier: f64,
        minimum_persistent_ttl: u32,
        warning_ledgers: u32,
        critical_ledgers: u32,
        ledgers_per_month: u32,
    ) -> Result<Self, ScenarioError> {
        let parameters = Self {
            fee_rate_per_byte,
            rent_write_fee_multiplier,
            minimum_persistent_ttl,
            warning_ledgers,
            critical_ledgers,
            ledgers_per_month,
        };
        parameters.validate()?;
        Ok(parameters)
    }

    /// Validates parameters, including values created by struct updates.
    pub fn validate(&self) -> Result<(), ScenarioError> {
        if !self.fee_rate_per_byte.is_finite() || self.fee_rate_per_byte < 0.0 {
            return Err(ScenarioError::InvalidParameters(
                "fee_rate_per_byte must be finite and non-negative".to_string(),
            ));
        }
        if !self.rent_write_fee_multiplier.is_finite() || self.rent_write_fee_multiplier <= 0.0 {
            return Err(ScenarioError::InvalidParameters(
                "rent_write_fee_multiplier must be finite and greater than 0".to_string(),
            ));
        }
        if self.minimum_persistent_ttl == 0 {
            return Err(ScenarioError::InvalidParameters(
                "minimum_persistent_ttl must be greater than 0".to_string(),
            ));
        }
        if self.ledgers_per_month == 0 {
            return Err(ScenarioError::InvalidParameters(
                "ledgers_per_month must be greater than 0".to_string(),
            ));
        }
        ThresholdConfig::try_new(self.warning_ledgers, self.critical_ledgers)
            .map_err(|error| ScenarioError::InvalidParameters(error.to_string()))?;
        if !self.monthly_cost_per_byte().is_finite() {
            return Err(ScenarioError::InvalidParameters(
                "rent parameters produce a non-finite monthly cost".to_string(),
            ));
        }
        Ok(())
    }

    fn thresholds(&self) -> ThresholdConfig {
        ThresholdConfig::new(self.warning_ledgers, self.critical_ledgers)
    }

    /// Expose the existing rent formula within the core for WASM analysis reuse.
    pub(crate) fn monthly_cost_per_byte(&self) -> f64 {
        self.fee_rate_per_byte * self.rent_write_fee_multiplier * self.ledgers_per_month as f64
            / self.minimum_persistent_ttl as f64
    }
}

/// Entry data required for an isolated scenario evaluation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScenarioEntry {
    /// Human-readable or base64 key identifier.
    pub key_identifier: String,
    /// Serialized ledger-entry size in bytes.
    pub byte_size: usize,
    /// Remaining ledgers at the time of the source scan.
    pub remaining_ledgers: i64,
}

impl ScenarioEntry {
    /// Creates a scenario input from a scan's per-entry measurements.
    pub fn new(
        key_identifier: impl Into<String>,
        byte_size: usize,
        remaining_ledgers: i64,
    ) -> Self {
        Self {
            key_identifier: key_identifier.into(),
            byte_size,
            remaining_ledgers,
        }
    }
}

/// Errors raised while validating scenario inputs.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ScenarioError {
    /// A rent parameter or threshold is invalid.
    #[error("Invalid protocol scenario parameters: {0}")]
    InvalidParameters(String),
}

/// Per-entry baseline and simulated cost and risk impact.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScenarioEntryImpact {
    /// Human-readable or base64 key identifier.
    pub key_identifier: String,
    /// Estimated monthly rent in stroops with baseline parameters.
    pub baseline_monthly_upkeep_stroops: f64,
    /// Estimated monthly rent in stroops with simulated parameters.
    pub simulated_monthly_upkeep_stroops: f64,
    /// Risk classification using the baseline horizon thresholds.
    pub baseline_status: RiskLevel,
    /// Risk classification using the simulated horizon thresholds.
    pub simulated_status: RiskLevel,
    /// True if the entry falls below the simulated warning horizon.
    pub breaches_warning_threshold: bool,
    /// True if the entry falls below the simulated critical horizon.
    pub breaches_critical_threshold: bool,
}

/// Aggregate result comparing baseline network parameters with a scenario.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProtocolScenarioReport {
    /// Sum of estimated monthly upkeep using baseline parameters.
    pub baseline_monthly_upkeep_stroops: f64,
    /// Sum of estimated monthly upkeep using simulated parameters.
    pub simulated_monthly_upkeep_stroops: f64,
    /// Percentage change from baseline, or `None` when a zero baseline makes
    /// the percentage undefined.
    pub monthly_upkeep_change_percent: Option<f64>,
    /// Per-entry cost and risk impacts.
    pub entries: Vec<ScenarioEntryImpact>,
}

/// Compares per-entry upkeep and TTL risk under baseline and simulated rules.
pub fn evaluate_protocol_scenario(
    entries: &[ScenarioEntry],
    baseline: &NetworkRentParameters,
    simulated: &NetworkRentParameters,
) -> Result<ProtocolScenarioReport, ScenarioError> {
    baseline.validate()?;
    simulated.validate()?;
    let baseline_cost_per_byte = baseline.monthly_cost_per_byte();
    let simulated_cost_per_byte = simulated.monthly_cost_per_byte();
    let baseline_thresholds = baseline.thresholds();
    let simulated_thresholds = simulated.thresholds();

    let entries: Vec<ScenarioEntryImpact> = entries
        .iter()
        .map(|entry| {
            let byte_size = entry.byte_size as f64;
            let baseline_monthly_upkeep_stroops = byte_size * baseline_cost_per_byte;
            let simulated_monthly_upkeep_stroops = byte_size * simulated_cost_per_byte;
            if !baseline_monthly_upkeep_stroops.is_finite()
                || !simulated_monthly_upkeep_stroops.is_finite()
            {
                return Err(ScenarioError::InvalidParameters(format!(
                    "estimated cost for entry '{}' is not finite",
                    entry.key_identifier
                )));
            }
            Ok(ScenarioEntryImpact {
                key_identifier: entry.key_identifier.clone(),
                baseline_monthly_upkeep_stroops,
                simulated_monthly_upkeep_stroops,
                baseline_status: evaluate_risk(entry.remaining_ledgers, &baseline_thresholds),
                simulated_status: evaluate_risk(entry.remaining_ledgers, &simulated_thresholds),
                breaches_warning_threshold: entry.remaining_ledgers
                    < simulated.warning_ledgers as i64,
                breaches_critical_threshold: entry.remaining_ledgers
                    < simulated.critical_ledgers as i64,
            })
        })
        .collect::<Result<_, _>>()?;

    let baseline_monthly_upkeep_stroops = entries
        .iter()
        .map(|entry| entry.baseline_monthly_upkeep_stroops)
        .sum::<f64>();
    let simulated_monthly_upkeep_stroops = entries
        .iter()
        .map(|entry| entry.simulated_monthly_upkeep_stroops)
        .sum::<f64>();
    if !baseline_monthly_upkeep_stroops.is_finite() || !simulated_monthly_upkeep_stroops.is_finite()
    {
        return Err(ScenarioError::InvalidParameters(
            "aggregate monthly upkeep is not finite".to_string(),
        ));
    }
    let monthly_upkeep_change_percent = if baseline_monthly_upkeep_stroops == 0.0 {
        if simulated_monthly_upkeep_stroops == 0.0 {
            Some(0.0)
        } else {
            None
        }
    } else {
        Some(
            (simulated_monthly_upkeep_stroops - baseline_monthly_upkeep_stroops)
                / baseline_monthly_upkeep_stroops
                * 100.0,
        )
    };

    Ok(ProtocolScenarioReport {
        baseline_monthly_upkeep_stroops,
        simulated_monthly_upkeep_stroops,
        monthly_upkeep_change_percent,
        entries,
    })
}
