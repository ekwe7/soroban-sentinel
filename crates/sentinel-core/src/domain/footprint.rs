//! Invocation footprint TTL validation.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use stellar_xdr::curr::{LedgerFootprint, LedgerKey, WriteXdr};

use super::{calculate_remaining_ledgers, RiskLevel, StorageEntry};

/// Whether a footprint dependency is read-only or may be mutated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FootprintAccess {
    ReadOnly,
    ReadWrite,
}

/// A single footprint key that cannot safely survive the invocation horizon.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AtRiskFootprint {
    pub key_xdr: String,
    pub access: FootprintAccess,
    pub remaining_ledgers: i64,
    pub live_until_ledger: u32,
}

/// Result of comparing invocation footprint keys with storage TTL horizons.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FootprintValidation {
    pub target_horizon: u32,
    pub at_risk: Vec<AtRiskFootprint>,
}

impl FootprintValidation {
    pub fn is_safe(&self) -> bool {
        self.at_risk.is_empty()
    }
}

/// Validates a Soroban invocation footprint against a required execution horizon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FootprintValidator {
    footprint: LedgerFootprint,
    target_horizon_ledgers: u32,
}

impl FootprintValidator {
    /// Creates a validator from an XDR invocation footprint and a required ledger horizon.
    pub fn new(footprint: LedgerFootprint, target_horizon_ledgers: u32) -> Self {
        Self {
            footprint,
            target_horizon_ledgers,
        }
    }

    /// Creates a validator from a list of ledger keys.
    /// Keys are treated as read-only by default.
    pub fn taking_keys(keys: &[LedgerKey], target_horizon_ledgers: u32) -> Self {
        use stellar_xdr::curr::VecM;
        let read_only = VecM::try_from(keys.to_vec()).expect("footprint key list is valid");
        Self::new(
            LedgerFootprint {
                read_only,
                read_write: VecM::default(),
            },
            target_horizon_ledgers,
        )
    }

    pub fn target_horizon_ledgers(&self) -> u32 {
        self.target_horizon_ledgers
    }

    pub fn footprint(&self) -> &LedgerFootprint {
        &self.footprint
    }

    /// Cross-references the footprint with a map of canonical ledger-key XDR to live-until ledger.
    pub fn validate_ttl_map(
        &self,
        latest_ledger: u32,
        ttl_by_key: &HashMap<String, u32>,
    ) -> FootprintValidation {
        let mut at_risk = Vec::new();

        for key in self.footprint.read_only.iter() {
            self.check_key(
                key,
                FootprintAccess::ReadOnly,
                latest_ledger,
                ttl_by_key,
                &mut at_risk,
            );
        }
        for key in self.footprint.read_write.iter() {
            self.check_key(
                key,
                FootprintAccess::ReadWrite,
                latest_ledger,
                ttl_by_key,
                &mut at_risk,
            );
        }

        at_risk.sort_by(|a, b| {
            a.key_xdr
                .cmp(&b.key_xdr)
                .then_with(|| a.access.cmp(&b.access))
        });
        at_risk.dedup_by(|a, b| a.key_xdr == b.key_xdr && a.access == b.access);

        FootprintValidation {
            target_horizon: self.target_horizon_ledgers,
            at_risk,
        }
    }

    fn check_key(
        &self,
        key: &LedgerKey,
        access: FootprintAccess,
        latest_ledger: u32,
        ttl_by_key: &HashMap<String, u32>,
        at_risk: &mut Vec<AtRiskFootprint>,
    ) {
        let Ok(key_xdr) = key.to_xdr_base64(stellar_xdr::curr::Limits::none()) else {
            return;
        };

        // Missing entries are unsafe: they can cause invocation failure.
        let live_until = ttl_by_key.get(&key_xdr).copied().unwrap_or(0);
        let remaining = calculate_remaining_ledgers(live_until, latest_ledger);
        let horizon = i64::from(self.target_horizon_ledgers);

        if remaining < horizon {
            at_risk.push(AtRiskFootprint {
                key_xdr,
                access,
                remaining_ledgers: remaining,
                live_until_ledger: live_until,
            });
        }
    }

    /// Cross-references canonical storage entries with the footprint.
    ///
    /// Entries should be keyed by their canonical base64 `LedgerKey` XDR.
    pub fn validate_storage_entries(
        &self,
        latest_ledger: u32,
        entries: &HashMap<String, StorageEntry>,
    ) -> FootprintValidation {
        let ttl_by_key = entries
            .iter()
            .map(|(key, entry)| (key.clone(), entry.live_until_ledger()))
            .collect();
        self.validate_ttl_map(latest_ledger, &ttl_by_key)
    }

    /// Backwards-compatible name for callers that use a required horizon as ledger lead time.
    pub fn validate(
        &self,
        latest_ledger: u32,
        ttl_by_key: &HashMap<String, u32>,
    ) -> FootprintValidation {
        self.validate_ttl_map(latest_ledger, ttl_by_key)
    }

    /// Classify an individual remaining horizon against the invocation buffer.
    pub fn classify(&self, remaining_ledgers: i64) -> RiskLevel {
        if remaining_ledgers <= 0 {
            RiskLevel::Archived
        } else if remaining_ledgers < i64::from(self.target_horizon_ledgers) {
            RiskLevel::Critical
        } else {
            RiskLevel::Ok
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use stellar_xdr::curr::{
        ContractDataDurability, Hash, LedgerKeyContractData, ScAddress, ScVal, VecM,
    };

    fn key(value: u8) -> LedgerKey {
        LedgerKey::ContractData(LedgerKeyContractData {
            contract: ScAddress::Contract(Hash([7; 32])),
            key: ScVal::U32(value as u32),
            durability: ContractDataDurability::Persistent,
        })
    }

    fn key_xdr(key: &LedgerKey) -> String {
        key.to_xdr_base64(stellar_xdr::curr::Limits::none())
            .unwrap()
    }

    #[test]
    fn detects_read_write_key_below_execution_horizon() {
        let k = key(1);
        let footprint = LedgerFootprint {
            read_only: VecM::default(),
            read_write: VecM::try_from(vec![k.clone()]).unwrap(),
        };
        let validator = FootprintValidator::new(footprint, 100);
        let mut ttls = HashMap::new();
        ttls.insert(key_xdr(&k), 1_000);

        let result = validator.validate_ttl_map(950, &ttls);
        assert!(!result.is_safe());
        assert_eq!(result.at_risk[0].access, FootprintAccess::ReadWrite);
    }

    #[test]
    fn missing_footprint_dependency_is_at_risk() {
        let validator = FootprintValidator::taking_keys(&[key(2)], 100);
        let result = validator.validate_ttl_map(1_000, &HashMap::new());
        assert_eq!(result.at_risk.len(), 1);
        assert_eq!(result.at_risk[0].access, FootprintAccess::ReadOnly);
    }

    #[test]
    fn key_with_sufficient_ttl_is_safe() {
        let k = key(3);
        let validator = FootprintValidator::taking_keys(std::slice::from_ref(&k), 100);
        let mut ttls = HashMap::new();
        ttls.insert(key_xdr(&k), 1_500);

        let result = validator.validate_ttl_map(1_000, &ttls);
        assert!(result.is_safe());
    }
}
