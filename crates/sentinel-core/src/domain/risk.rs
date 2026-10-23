//! Risk classification engine and threshold rules for Soroban storage entries.

use crate::error::{SentinelError, SentinelResult};
use serde::{Deserialize, Serialize};

/// Default warning threshold: 120,960 ledgers (~7 days at 5 seconds per ledger).
pub const DEFAULT_WARNING_LEDGERS: u32 = 120_960;

/// Default critical threshold: 34,560 ledgers (~2 days at 5 seconds per ledger).
pub const DEFAULT_CRITICAL_LEDGERS: u32 = 34_560;

/// Risk status classification of a storage entry's remaining lifespan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RiskLevel {
    /// Safe: entry has adequate remaining TTL horizon.
    Ok,
    /// Warning: entry has less than warning threshold ledgers remaining.
    Warn,
    /// Critical: entry is imminent risk of expiration.
    Critical,
    /// Archived: entry has already expired or has zero live-until ledger.
    Archived,
}

impl std::fmt::Display for RiskLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Ok => write!(f, "OK"),
            Self::Warn => write!(f, "WARN"),
            Self::Critical => write!(f, "CRITICAL"),
            Self::Archived => write!(f, "ARCHIVED"),
        }
    }
}

/// Configuration thresholds defining when an entry transitions into Warn or Critical states.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThresholdConfig {
    /// Remaining ledgers threshold below which an entry is classified as `Warn`.
    pub warning_ledgers: u32,
    /// Remaining ledgers threshold below which an entry is classified as `Critical`.
    pub critical_ledgers: u32,
}

impl ThresholdConfig {
    /// Constructs a new `ThresholdConfig` without validation (for backwards compatibility).
    pub fn new(warning_ledgers: u32, critical_ledgers: u32) -> Self {
        Self {
            warning_ledgers,
            critical_ledgers,
        }
    }

    /// Constructs and validates a new `ThresholdConfig`.
    pub fn try_new(warning_ledgers: u32, critical_ledgers: u32) -> SentinelResult<Self> {
        let config = Self {
            warning_ledgers,
            critical_ledgers,
        };
        config.validate()?;
        Ok(config)
    }

    /// Validates that thresholds are logically sound and non-contradictory.
    ///
    /// Requirements:
    /// - `warning_ledgers > 0`
    /// - `critical_ledgers > 0`
    /// - `critical_ledgers < warning_ledgers`
    pub fn validate(&self) -> SentinelResult<()> {
        if self.warning_ledgers == 0 {
            return Err(SentinelError::InvalidThresholdConfig(
                "Warning threshold must be greater than 0".to_string(),
            ));
        }
        if self.critical_ledgers == 0 {
            return Err(SentinelError::InvalidThresholdConfig(
                "Critical threshold must be greater than 0".to_string(),
            ));
        }
        if self.critical_ledgers >= self.warning_ledgers {
            return Err(SentinelError::InvalidThresholdConfig(format!(
                "Critical threshold ({}) must be strictly less than warning threshold ({})",
                self.critical_ledgers, self.warning_ledgers
            )));
        }
        Ok(())
    }
}

impl Default for ThresholdConfig {
    fn default() -> Self {
        Self {
            warning_ledgers: DEFAULT_WARNING_LEDGERS,
            critical_ledgers: DEFAULT_CRITICAL_LEDGERS,
        }
    }
}

/// Evaluates remaining ledgers against a `ThresholdConfig` to determine risk classification.
///
/// Uses direct 64-bit integer comparison to eliminate lossy truncation or integer overflow.
///
/// # Rules:
/// - If `remaining_ledgers <= 0` -> `RiskLevel::Archived`
/// - If `remaining_ledgers < config.critical_ledgers as i64` -> `RiskLevel::Critical`
/// - If `remaining_ledgers < config.warning_ledgers as i64` -> `RiskLevel::Warn`
/// - Otherwise -> `RiskLevel::Ok`
pub fn evaluate_risk(remaining_ledgers: i64, config: &ThresholdConfig) -> RiskLevel {
    if remaining_ledgers <= 0 {
        RiskLevel::Archived
    } else if remaining_ledgers < config.critical_ledgers as i64 {
        RiskLevel::Critical
    } else if remaining_ledgers < config.warning_ledgers as i64 {
        RiskLevel::Warn
    } else {
        RiskLevel::Ok
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_risk_evaluation_boundaries() {
        let config = ThresholdConfig::default();
        let crit = config.critical_ledgers;
        let warn = config.warning_ledgers;

        // Negative -> Archived
        assert_eq!(evaluate_risk(-1, &config), RiskLevel::Archived);
        assert_eq!(evaluate_risk(-500, &config), RiskLevel::Archived);

        // Zero -> Archived
        assert_eq!(evaluate_risk(0, &config), RiskLevel::Archived);

        // Below critical threshold -> Critical
        assert_eq!(evaluate_risk(1, &config), RiskLevel::Critical);
        assert_eq!(
            evaluate_risk((crit - 1) as i64, &config),
            RiskLevel::Critical
        );

        // Exact critical threshold -> Warn (since not strictly less than crit)
        assert_eq!(evaluate_risk(crit as i64, &config), RiskLevel::Warn);

        // Between critical and warning -> Warn
        assert_eq!(evaluate_risk((crit + 1) as i64, &config), RiskLevel::Warn);
        assert_eq!(evaluate_risk((warn - 1) as i64, &config), RiskLevel::Warn);

        // Exact warning threshold -> Ok (since not strictly less than warn)
        assert_eq!(evaluate_risk(warn as i64, &config), RiskLevel::Ok);

        // Well above warning -> Ok
        assert_eq!(evaluate_risk((warn + 1) as i64, &config), RiskLevel::Ok);
        assert_eq!(evaluate_risk(1_000_000, &config), RiskLevel::Ok);
    }

    #[test]
    fn test_custom_threshold_config() {
        let custom = ThresholdConfig::new(100, 20);

        assert_eq!(evaluate_risk(0, &custom), RiskLevel::Archived);
        assert_eq!(evaluate_risk(19, &custom), RiskLevel::Critical);
        assert_eq!(evaluate_risk(20, &custom), RiskLevel::Warn);
        assert_eq!(evaluate_risk(99, &custom), RiskLevel::Warn);
        assert_eq!(evaluate_risk(100, &custom), RiskLevel::Ok);
        assert_eq!(evaluate_risk(101, &custom), RiskLevel::Ok);
    }

    #[test]
    fn test_threshold_validation_valid_and_invalid() {
        // Valid
        assert!(ThresholdConfig::try_new(100, 50).is_ok());
        assert!(ThresholdConfig::default().validate().is_ok());

        // Contradictory: critical == warning
        let err = ThresholdConfig::try_new(100, 100).unwrap_err();
        assert!(matches!(err, SentinelError::InvalidThresholdConfig(_)));

        // Contradictory: critical > warning
        let err = ThresholdConfig::try_new(50, 100).unwrap_err();
        assert!(matches!(err, SentinelError::InvalidThresholdConfig(_)));

        // Invalid: warning is 0
        let err = ThresholdConfig::try_new(0, 50).unwrap_err();
        assert!(matches!(err, SentinelError::InvalidThresholdConfig(_)));

        // Invalid: critical is 0
        let err = ThresholdConfig::try_new(100, 0).unwrap_err();
        assert!(matches!(err, SentinelError::InvalidThresholdConfig(_)));
    }

    #[test]
    fn test_risk_evaluation_64bit_no_truncation() {
        let config = ThresholdConfig::default();
        // Values exceeding u32::MAX (4,294,967,295)
        let huge_value: i64 = 5_000_000_000;
        assert_eq!(evaluate_risk(huge_value, &config), RiskLevel::Ok);

        let i64_max = i64::MAX;
        assert_eq!(evaluate_risk(i64_max, &config), RiskLevel::Ok);
    }

    #[test]
    fn test_risk_level_display() {
        assert_eq!(RiskLevel::Ok.to_string(), "OK");
        assert_eq!(RiskLevel::Warn.to_string(), "WARN");
        assert_eq!(RiskLevel::Critical.to_string(), "CRITICAL");
        assert_eq!(RiskLevel::Archived.to_string(), "ARCHIVED");
    }

    #[test]
    fn test_risk_level_serde() {
        assert_eq!(serde_json::to_string(&RiskLevel::Ok).unwrap(), "\"OK\"");
        assert_eq!(serde_json::to_string(&RiskLevel::Warn).unwrap(), "\"WARN\"");
        assert_eq!(
            serde_json::to_string(&RiskLevel::Critical).unwrap(),
            "\"CRITICAL\""
        );
        assert_eq!(
            serde_json::to_string(&RiskLevel::Archived).unwrap(),
            "\"ARCHIVED\""
        );

        let parsed: RiskLevel = serde_json::from_str("\"CRITICAL\"").unwrap();
        assert_eq!(parsed, RiskLevel::Critical);
    }
}
