//! Stateful alert hysteresis for suppressing noisy TTL risk transitions.
//!
//! This is an in-memory domain filter. Callers should retain one filter for the
//! lifetime of a monitoring session and pass a stable identifier for each key.

use std::collections::HashMap;

use crate::domain::risk::{evaluate_risk, RiskLevel, ThresholdConfig};

/// Hysteresis behavior for warning and recovery transitions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HysteresisConfig {
    /// Risk thresholds used to classify each remaining-ledger evaluation.
    pub thresholds: ThresholdConfig,
    /// Consecutive warning evaluations required before opening an alert.
    pub warning_breach_scans: usize,
    /// Consecutive healthy evaluations required before clearing an alert.
    pub stable_scans: usize,
}

impl Default for HysteresisConfig {
    fn default() -> Self {
        Self {
            thresholds: ThresholdConfig::default(),
            warning_breach_scans: 2,
            stable_scans: 2,
        }
    }
}

/// Action a caller should take after evaluating a storage key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlertAction {
    /// Emit an alert for a newly active or immediately escalated risk.
    FireAlert,
    /// Keep the current notification state without emitting another event.
    Suppress,
    /// Clear an active alert after enough consecutive healthy evaluations.
    Clear,
}

#[derive(Debug, Clone, Copy)]
struct KeyState {
    active_risk: RiskLevel,
    warning_breaches: usize,
    stable_scans: usize,
}

impl Default for KeyState {
    fn default() -> Self {
        Self {
            active_risk: RiskLevel::Ok,
            warning_breaches: 0,
            stable_scans: 0,
        }
    }
}

/// Tracks alert state independently for each storage key.
#[derive(Debug, Clone)]
pub struct HysteresisFilter {
    config: HysteresisConfig,
    states: HashMap<String, KeyState>,
}

impl Default for HysteresisFilter {
    fn default() -> Self {
        Self::new()
    }
}

impl HysteresisFilter {
    /// Creates a filter with the default risk thresholds and two-scan transitions.
    pub fn new() -> Self {
        Self::with_config(HysteresisConfig::default())
    }

    /// Creates a filter with custom thresholds and transition counts.
    ///
    /// A configured count of zero is treated as one, so every transition still
    /// requires at least one evaluation.
    pub fn with_config(mut config: HysteresisConfig) -> Self {
        config.warning_breach_scans = config.warning_breach_scans.max(1);
        config.stable_scans = config.stable_scans.max(1);
        Self {
            config,
            states: HashMap::new(),
        }
    }

    /// Evaluates a key's remaining TTL and returns the notification action.
    ///
    /// Warning transitions require consecutive warning evaluations. Critical
    /// and archived evaluations fire immediately. An active alert clears only
    /// after `stable_scans` consecutive evaluations classify the key as `Ok`.
    pub fn update(&mut self, key: impl Into<String>, remaining_ledgers: i64) -> AlertAction {
        let evaluated_risk = evaluate_risk(remaining_ledgers, &self.config.thresholds);
        let state = self.states.entry(key.into()).or_default();

        if evaluated_risk.is_critical_or_worse() {
            state.warning_breaches = 0;
            state.stable_scans = 0;
            if evaluated_risk > state.active_risk {
                state.active_risk = evaluated_risk;
                return AlertAction::FireAlert;
            }
            return AlertAction::Suppress;
        }

        if evaluated_risk == RiskLevel::Warn {
            state.stable_scans = 0;
            if state.active_risk == RiskLevel::Ok {
                state.warning_breaches = state.warning_breaches.saturating_add(1);
                if state.warning_breaches >= self.config.warning_breach_scans {
                    state.active_risk = RiskLevel::Warn;
                    state.warning_breaches = 0;
                    return AlertAction::FireAlert;
                }
            } else {
                state.warning_breaches = 0;
            }
            return AlertAction::Suppress;
        }

        state.warning_breaches = 0;
        if state.active_risk == RiskLevel::Ok {
            state.stable_scans = 0;
            return AlertAction::Suppress;
        }

        state.stable_scans = state.stable_scans.saturating_add(1);
        if state.stable_scans >= self.config.stable_scans {
            state.active_risk = RiskLevel::Ok;
            state.stable_scans = 0;
            return AlertAction::Clear;
        }
        AlertAction::Suppress
    }

    /// Returns the currently active risk level for a key, if it has been seen.
    pub fn active_risk(&self, key: &str) -> Option<RiskLevel> {
        self.states.get(key).map(|state| state.active_risk)
    }
}
