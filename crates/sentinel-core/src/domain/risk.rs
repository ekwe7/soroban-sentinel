//! Risk classification engine and threshold rules for Soroban storage entries.

use crate::domain::Durability;
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

impl RiskLevel {
    /// Whether this risk needs an immediate alert, bypassing warning hysteresis.
    pub const fn is_critical_or_worse(self) -> bool {
        matches!(self, Self::Critical | Self::Archived)
    }
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

fn validate_threshold_bounds(
    scope: Option<&str>,
    warning_ledgers: u32,
    critical_ledgers: u32,
) -> SentinelResult<()> {
    if warning_ledgers == 0 {
        let message = match scope {
            Some(name) => format!("{name} override: warning threshold must be greater than 0"),
            None => "Warning threshold must be greater than 0".to_string(),
        };
        return Err(SentinelError::InvalidThresholdConfig(message));
    }
    if critical_ledgers == 0 {
        let message = match scope {
            Some(name) => format!("{name} override: critical threshold must be greater than 0"),
            None => "Critical threshold must be greater than 0".to_string(),
        };
        return Err(SentinelError::InvalidThresholdConfig(message));
    }
    if critical_ledgers >= warning_ledgers {
        let message = match scope {
            Some(name) => format!(
                "{name} override: critical threshold ({critical_ledgers}) must be strictly less than warning threshold ({warning_ledgers})"
            ),
            None => format!(
                "Critical threshold ({critical_ledgers}) must be strictly less than warning threshold ({warning_ledgers})"
            ),
        };
        return Err(SentinelError::InvalidThresholdConfig(message));
    }
    Ok(())
}

/// Optional per-durability threshold override defining when an entry transitions into Warn or Critical states.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThresholdOverride {
    /// Remaining ledgers threshold below which an entry is classified as `Warn`.
    pub warning_ledgers: u32,
    /// Remaining ledgers threshold below which an entry is classified as `Critical`.
    pub critical_ledgers: u32,
}

impl ThresholdOverride {
    /// Constructs a new `ThresholdOverride` without validation.
    pub fn new(warning_ledgers: u32, critical_ledgers: u32) -> Self {
        Self {
            warning_ledgers,
            critical_ledgers,
        }
    }

    /// Constructs and validates a new `ThresholdOverride`.
    pub fn try_new(warning_ledgers: u32, critical_ledgers: u32) -> SentinelResult<Self> {
        let override_cfg = Self::new(warning_ledgers, critical_ledgers);
        override_cfg.validate()?;
        Ok(override_cfg)
    }

    /// Validates that override thresholds are logically sound and non-zero.
    ///
    /// Requirements:
    /// - `warning_ledgers > 0`
    /// - `critical_ledgers > 0`
    /// - `critical_ledgers < warning_ledgers`
    pub fn validate(&self) -> SentinelResult<()> {
        validate_threshold_bounds(None, self.warning_ledgers, self.critical_ledgers)
    }
}

/// Configuration thresholds defining when an entry transitions into Warn or Critical states.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThresholdConfig {
    /// Remaining ledgers threshold below which an entry is classified as `Warn`.
    pub warning_ledgers: u32,
    /// Remaining ledgers threshold below which an entry is classified as `Critical`.
    pub critical_ledgers: u32,
    /// Optional threshold override for `Durability::Instance` entries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance: Option<ThresholdOverride>,
    /// Optional threshold override for `Durability::Code` entries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<ThresholdOverride>,
    /// Optional threshold override for `Durability::Persistent` entries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub persistent: Option<ThresholdOverride>,
}

impl ThresholdConfig {
    /// Constructs a new `ThresholdConfig` without validation (for backwards compatibility).
    pub fn new(warning_ledgers: u32, critical_ledgers: u32) -> Self {
        Self {
            warning_ledgers,
            critical_ledgers,
            instance: None,
            code: None,
            persistent: None,
        }
    }

    /// Constructs and validates a new `ThresholdConfig`.
    pub fn try_new(warning_ledgers: u32, critical_ledgers: u32) -> SentinelResult<Self> {
        let config = Self::new(warning_ledgers, critical_ledgers);
        config.validate()?;
        Ok(config)
    }

    /// Sets an optional threshold override for `Durability::Instance` entries.
    pub fn with_instance_override(mut self, override_cfg: ThresholdOverride) -> Self {
        self.instance = Some(override_cfg);
        self
    }

    /// Sets an optional threshold override for `Durability::Code` entries.
    pub fn with_code_override(mut self, override_cfg: ThresholdOverride) -> Self {
        self.code = Some(override_cfg);
        self
    }

    /// Sets an optional threshold override for `Durability::Persistent` entries.
    pub fn with_persistent_override(mut self, override_cfg: ThresholdOverride) -> Self {
        self.persistent = Some(override_cfg);
        self
    }

    /// Resolves the effective threshold configuration for a specific storage durability tier.
    ///
    /// Falls back to the global `warning_ledgers` and `critical_ledgers` when no override
    /// is configured for `durability`.
    pub fn threshold_for(&self, durability: Durability) -> ThresholdConfig {
        let override_cfg = match durability {
            Durability::Instance => self.instance,
            Durability::Code => self.code,
            Durability::Persistent => self.persistent,
            Durability::Temporary => None,
        };

        match override_cfg {
            Some(ov) => ThresholdConfig::new(ov.warning_ledgers, ov.critical_ledgers),
            None => ThresholdConfig::new(self.warning_ledgers, self.critical_ledgers),
        }
    }

    /// Validates that global thresholds and all configured per-durability overrides
    /// are logically sound and non-contradictory.
    ///
    /// Requirements for global thresholds and each configured override:
    /// - `warning_ledgers > 0`
    /// - `critical_ledgers > 0`
    /// - `critical_ledgers < warning_ledgers`
    pub fn validate(&self) -> SentinelResult<()> {
        validate_threshold_bounds(None, self.warning_ledgers, self.critical_ledgers)?;
        if let Some(ov) = self.instance {
            validate_threshold_bounds(Some("instance"), ov.warning_ledgers, ov.critical_ledgers)?;
        }
        if let Some(ov) = self.code {
            validate_threshold_bounds(Some("code"), ov.warning_ledgers, ov.critical_ledgers)?;
        }
        if let Some(ov) = self.persistent {
            validate_threshold_bounds(Some("persistent"), ov.warning_ledgers, ov.critical_ledgers)?;
        }
        Ok(())
    }
}

impl Default for ThresholdConfig {
    fn default() -> Self {
        Self {
            warning_ledgers: DEFAULT_WARNING_LEDGERS,
            critical_ledgers: DEFAULT_CRITICAL_LEDGERS,
            instance: None,
            code: None,
            persistent: None,
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

/// Evaluates remaining ledgers for a specific storage `Durability` tier against a `ThresholdConfig`.
///
/// Resolves the effective thresholds via [`ThresholdConfig::threshold_for`] and delegates to
/// [`evaluate_risk`].
pub fn evaluate_risk_for_durability(
    remaining_ledgers: i64,
    durability: Durability,
    config: &ThresholdConfig,
) -> RiskLevel {
    let effective = config.threshold_for(durability);
    evaluate_risk(remaining_ledgers, &effective)
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

    #[test]
    fn test_threshold_for_falls_back_to_global_when_overrides_unset() {
        let config = ThresholdConfig::new(100_000, 25_000);
        let expected = ThresholdConfig::new(100_000, 25_000);

        assert_eq!(config.threshold_for(Durability::Instance), expected);
        assert_eq!(config.threshold_for(Durability::Code), expected);
        assert_eq!(config.threshold_for(Durability::Persistent), expected);
        assert_eq!(config.threshold_for(Durability::Temporary), expected);

        for durability in [
            Durability::Instance,
            Durability::Code,
            Durability::Persistent,
            Durability::Temporary,
        ] {
            assert_eq!(
                evaluate_risk_for_durability(0, durability, &config),
                RiskLevel::Archived
            );
            assert_eq!(
                evaluate_risk_for_durability(24_999, durability, &config),
                RiskLevel::Critical
            );
            assert_eq!(
                evaluate_risk_for_durability(25_000, durability, &config),
                RiskLevel::Warn
            );
            assert_eq!(
                evaluate_risk_for_durability(99_999, durability, &config),
                RiskLevel::Warn
            );
            assert_eq!(
                evaluate_risk_for_durability(100_000, durability, &config),
                RiskLevel::Ok
            );
        }
    }

    #[test]
    fn test_independent_risk_classification_per_durability() {
        // Global fallback: warn < 100k, crit < 20k
        // Code: stricter (warn < 500k, crit < 200k)
        // Instance: moderate (warn < 300k, crit < 100k)
        // Persistent: relaxed (warn < 50k, crit < 10k)
        let config = ThresholdConfig::new(100_000, 20_000)
            .with_code_override(ThresholdOverride::new(500_000, 200_000))
            .with_instance_override(ThresholdOverride::new(300_000, 100_000))
            .with_persistent_override(ThresholdOverride::new(50_000, 10_000));

        assert!(config.validate().is_ok());

        // At 150,000 remaining ledgers:
        // - Code (< 200k crit) -> Critical
        // - Instance (>= 100k crit, < 300k warn) -> Warn
        // - Persistent (>= 50k warn) -> Ok
        // - Temporary (falls back to global: >= 100k warn) -> Ok
        assert_eq!(
            evaluate_risk_for_durability(150_000, Durability::Code, &config),
            RiskLevel::Critical
        );
        assert_eq!(
            evaluate_risk_for_durability(150_000, Durability::Instance, &config),
            RiskLevel::Warn
        );
        assert_eq!(
            evaluate_risk_for_durability(150_000, Durability::Persistent, &config),
            RiskLevel::Ok
        );
        assert_eq!(
            evaluate_risk_for_durability(150_000, Durability::Temporary, &config),
            RiskLevel::Ok
        );

        // Legacy evaluate_risk continues to use global bounds unaffected by overrides.
        assert_eq!(evaluate_risk(150_000, &config), RiskLevel::Ok);
        assert_eq!(evaluate_risk(50_000, &config), RiskLevel::Warn);
        assert_eq!(evaluate_risk(15_000, &config), RiskLevel::Critical);

        // Zero and negative remain Archived across all durabilities.
        for durability in [
            Durability::Instance,
            Durability::Code,
            Durability::Persistent,
            Durability::Temporary,
        ] {
            assert_eq!(
                evaluate_risk_for_durability(0, durability, &config),
                RiskLevel::Archived
            );
            assert_eq!(
                evaluate_risk_for_durability(-42, durability, &config),
                RiskLevel::Archived
            );
        }
    }

    #[test]
    fn test_partial_override_falls_back_for_unset_durabilities() {
        let config = ThresholdConfig::new(100_000, 30_000)
            .with_code_override(ThresholdOverride::new(400_000, 150_000));

        assert_eq!(
            config.threshold_for(Durability::Code),
            ThresholdConfig::new(400_000, 150_000)
        );
        assert_eq!(
            config.threshold_for(Durability::Instance),
            ThresholdConfig::new(100_000, 30_000)
        );
        assert_eq!(
            config.threshold_for(Durability::Persistent),
            ThresholdConfig::new(100_000, 30_000)
        );

        // At 120,000 remaining ledgers: Code is Critical, Instance & Persistent are Ok.
        assert_eq!(
            evaluate_risk_for_durability(120_000, Durability::Code, &config),
            RiskLevel::Critical
        );
        assert_eq!(
            evaluate_risk_for_durability(120_000, Durability::Instance, &config),
            RiskLevel::Ok
        );
        assert_eq!(
            evaluate_risk_for_durability(120_000, Durability::Persistent, &config),
            RiskLevel::Ok
        );
    }

    #[test]
    fn test_override_validation_bounds() {
        // ThresholdOverride::try_new validation
        assert!(ThresholdOverride::try_new(200, 100).is_ok());
        assert!(matches!(
            ThresholdOverride::try_new(0, 100),
            Err(SentinelError::InvalidThresholdConfig(_))
        ));
        assert!(matches!(
            ThresholdOverride::try_new(100, 0),
            Err(SentinelError::InvalidThresholdConfig(_))
        ));
        assert!(matches!(
            ThresholdOverride::try_new(100, 100),
            Err(SentinelError::InvalidThresholdConfig(_))
        ));
        assert!(matches!(
            ThresholdOverride::try_new(50, 100),
            Err(SentinelError::InvalidThresholdConfig(_))
        ));

        let invalid_pairs = [(0, 10), (10, 0), (100, 100), (50, 100)];

        for (warn, crit) in invalid_pairs {
            let bad = ThresholdOverride::new(warn, crit);

            let inst_cfg = ThresholdConfig::default().with_instance_override(bad);
            assert!(matches!(
                inst_cfg.validate(),
                Err(SentinelError::InvalidThresholdConfig(msg)) if msg.contains("instance")
            ));

            let code_cfg = ThresholdConfig::default().with_code_override(bad);
            assert!(matches!(
                code_cfg.validate(),
                Err(SentinelError::InvalidThresholdConfig(msg)) if msg.contains("code")
            ));

            let pers_cfg = ThresholdConfig::default().with_persistent_override(bad);
            assert!(matches!(
                pers_cfg.validate(),
                Err(SentinelError::InvalidThresholdConfig(msg)) if msg.contains("persistent")
            ));
        }
    }

    #[test]
    fn test_threshold_config_serde_backwards_compatibility() {
        // Default serialization omits unset override fields
        let default_cfg = ThresholdConfig::default();
        let json = serde_json::to_string(&default_cfg).unwrap();
        assert_eq!(
            json,
            format!(
                r#"{{"warning_ledgers":{},"critical_ledgers":{}}}"#,
                DEFAULT_WARNING_LEDGERS, DEFAULT_CRITICAL_LEDGERS
            )
        );

        // Deserializing legacy JSON without override keys works via #[serde(default)]
        let legacy_json = r#"{"warning_ledgers":120960,"critical_ledgers":34560}"#;
        let parsed: ThresholdConfig = serde_json::from_str(legacy_json).unwrap();
        assert_eq!(parsed, default_cfg);
        assert_eq!(parsed.instance, None);
        assert_eq!(parsed.code, None);
        assert_eq!(parsed.persistent, None);

        // Round-trip with overrides populated
        let with_overrides = ThresholdConfig::new(100_000, 20_000)
            .with_instance_override(ThresholdOverride::new(250_000, 80_000))
            .with_code_override(ThresholdOverride::new(400_000, 120_000));
        let serialized = serde_json::to_string(&with_overrides).unwrap();
        assert!(!serialized.contains("persistent"));
        let deserialized: ThresholdConfig = serde_json::from_str(&serialized).unwrap();
        assert_eq!(deserialized, with_overrides);
    }
}
