//! Pure TTL extension recommendation planning.

use serde::{Deserialize, Serialize};

use super::{Durability, RiskLevel};

pub const CONSERVATIVE_DEFAULT_MAX_ENTRY_TTL: u32 = 3110400;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "action")]
pub enum RecommendationAction {
    Extend {
        extend_to: u32,
        ledgers_to_extend: u32,
    },
    Restore,
    Unrecoverable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TTLRecommendation {
    pub key: String,
    pub durability: Durability,
    pub status: RiskLevel,
    pub action: RecommendationAction,
    pub network_limit_checked: bool,
    pub clamped: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecommendationPlan {
    pub target_days: u32,
    pub target_ledgers: u32,
    pub max_entry_ttl: u32,
    pub network_limit_checked: bool,
    pub recommendations: Vec<TTLRecommendation>,
}

pub fn calculate_extend_to(
    latest_ledger: u32,
    target_days: u32,
    ledger_velocity_seconds: f64,
    max_entry_ttl: Option<u32>,
) -> (u32, u32, bool) {
    let velocity = if ledger_velocity_seconds.is_finite() && ledger_velocity_seconds > 0.0 {
        ledger_velocity_seconds
    } else {
        5.0
    };
    let target_ledgers = ((target_days as f64 * 86400.0) / velocity)
        .round()
        .clamp(1.0, u32::MAX as f64) as u32;
    let desired = latest_ledger.saturating_add(target_ledgers);
    let (effective_max, checked) = match max_entry_ttl {
        Some(limit) => (limit, true),
        None => (CONSERVATIVE_DEFAULT_MAX_ENTRY_TTL, false),
    };
    let extend_to = desired.min(effective_max);
    (extend_to, target_ledgers, checked)
}

pub fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

pub fn build_recommendation_plan(
    entries: &[crate::usecases::StorageEntryReport],
    latest_ledger: u32,
    target_days: u32,
    ledger_velocity_seconds: f64,
    max_entry_ttl: Option<u32>,
) -> RecommendationPlan {
    let (target_extend_to, target_ledgers, limit_checked) = calculate_extend_to(
        latest_ledger,
        target_days,
        ledger_velocity_seconds,
        max_entry_ttl,
    );

    let recommendations = entries
        .iter()
        .filter_map(|entry| match entry.status {
            RiskLevel::Archived => Some(TTLRecommendation {
                key: entry.key_identifier.clone(),
                durability: entry.durability,
                status: entry.status,
                action: RecommendationAction::Restore,
                network_limit_checked: limit_checked,
                clamped: false,
            }),
            RiskLevel::Warn | RiskLevel::Critical => {
                if matches!(entry.durability, Durability::Temporary) {
                    return Some(TTLRecommendation {
                        key: entry.key_identifier.clone(),
                        durability: entry.durability,
                        status: entry.status,
                        action: RecommendationAction::Unrecoverable,
                        network_limit_checked: limit_checked,
                        clamped: false,
                    });
                }
                let desired = target_extend_to.max(entry.live_until_ledger);
                let difference = desired.saturating_sub(entry.live_until_ledger);
                let clamped = max_entry_ttl
                    .is_some_and(|limit| desired >= limit && entry.live_until_ledger < limit);
                Some(TTLRecommendation {
                    key: entry.key_identifier.clone(),
                    durability: entry.durability,
                    status: entry.status,
                    action: RecommendationAction::Extend {
                        extend_to: desired,
                        ledgers_to_extend: difference,
                    },
                    network_limit_checked: limit_checked,
                    clamped,
                })
            }
            RiskLevel::Ok => None,
        })
        .collect();

    RecommendationPlan {
        target_days,
        target_ledgers,
        max_entry_ttl: max_entry_ttl.unwrap_or(CONSERVATIVE_DEFAULT_MAX_ENTRY_TTL),
        network_limit_checked: limit_checked,
        recommendations,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boundary_calculation_and_clamp() {
        let (extend_to, ledgers, checked) = calculate_extend_to(100, 30, 5.0, Some(500));
        assert_eq!(ledgers, 518400);
        assert_eq!(extend_to, 500);
        assert!(checked);

        let (extend_to, _, checked) = calculate_extend_to(100, 30, 5.0, None);
        assert_eq!(
            extend_to,
            CONSERVATIVE_DEFAULT_MAX_ENTRY_TTL.min(100 + 518400)
        );
        assert!(!checked);
    }

    #[test]
    fn hostile_key_shell_quoting_is_safe() {
        let quoted = shell_quote("key $(touch pwn); 'quote'");
        assert_eq!(quoted, "'key $(touch pwn); '\"'\"'quote'\"'\"''");
    }
}
