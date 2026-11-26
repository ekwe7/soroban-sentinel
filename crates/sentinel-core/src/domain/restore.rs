//! Pure planning for CAP-0046 state restoration footprints.
//!
//! Archived persistent entries cannot be extended; they must be restored with
//! a `RestoreFootprintOp`. This module builds the deterministic read-write
//! footprint and the corresponding unsigned operation without performing I/O.

use std::collections::HashSet;

use stellar_xdr::curr::{
    ExtensionPoint, LedgerFootprint, LedgerKey, Limits, Operation, OperationBody,
    RestoreFootprintOp, VecM, WriteXdr,
};
use thiserror::Error;

/// Errors raised while preparing a state restoration footprint.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RestoreError {
    /// The supplied key set could not form a valid footprint.
    #[error("Invalid restore footprint: {0}")]
    InvalidFootprint(String),

    /// XDR footprint encoding failed.
    #[error("Failed to encode restore footprint: {0}")]
    Xdr(String),
}

/// A deduplicated read-write footprint ready for a `RestoreFootprintOp`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestorePlan {
    /// Ledger keys to restore, in deterministic order.
    pub keys: Vec<LedgerKey>,
    /// Soroban footprint with every key in `read_write`.
    pub footprint: LedgerFootprint,
}

impl RestorePlan {
    /// Encodes the footprint as base64 XDR.
    pub fn to_xdr_base64(&self) -> Result<String, RestoreError> {
        self.footprint
            .to_xdr_base64(Limits::none())
            .map_err(|error| RestoreError::Xdr(error.to_string()))
    }

    /// Returns the protocol V0 restore operation for this footprint.
    pub fn operation(&self) -> Operation {
        Operation {
            source_account: None,
            body: OperationBody::RestoreFootprint(RestoreFootprintOp {
                ext: ExtensionPoint::V0,
            }),
        }
    }
}

/// Builds a deterministic restore footprint from ledger keys.
///
/// Duplicate keys are removed and the remainder are sorted. Restore operations
/// require the archived keys to be read-write, so `read_only` is always empty.
pub fn plan_restore_footprint(keys: &[LedgerKey]) -> Result<RestorePlan, RestoreError> {
    if keys.is_empty() {
        return Err(RestoreError::InvalidFootprint(
            "at least one ledger key is required".to_string(),
        ));
    }

    let mut seen = HashSet::with_capacity(keys.len());
    let mut unique: Vec<LedgerKey> = keys
        .iter()
        .filter(|key| seen.insert((*key).clone()))
        .cloned()
        .collect();
    unique.sort_unstable();

    let read_write = VecM::try_from(unique.clone())
        .map_err(|error| RestoreError::InvalidFootprint(error.to_string()))?;

    Ok(RestorePlan {
        keys: unique,
        footprint: LedgerFootprint {
            read_only: VecM::default(),
            read_write,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use stellar_xdr::curr::{
        ContractDataDurability, Hash, LedgerKeyContractData, ReadXdr, ScAddress, ScVal,
    };

    fn contract_data_key(seed: u8) -> LedgerKey {
        LedgerKey::ContractData(LedgerKeyContractData {
            contract: ScAddress::Contract(Hash([seed; 32])),
            key: ScVal::U32(seed as u32),
            durability: ContractDataDurability::Persistent,
        })
    }

    #[test]
    fn plan_restore_footprint_deduplicates_and_uses_read_write() {
        let key = contract_data_key(1);
        let plan = plan_restore_footprint(&[key.clone(), key.clone()]).unwrap();

        assert_eq!(plan.keys.len(), 1);
        assert!(plan.footprint.read_only.is_empty());
        assert_eq!(plan.footprint.read_write.len(), 1);
        assert_eq!(plan.footprint.read_write[0], key);
    }

    #[test]
    fn plan_restore_footprint_rejects_empty_keys() {
        assert!(plan_restore_footprint(&[]).is_err());
    }

    #[test]
    fn restore_plan_operation_and_xdr_roundtrip() {
        let key = contract_data_key(2);
        let plan = plan_restore_footprint(&[key]).unwrap();
        let operation = plan.operation();

        assert!(matches!(
            operation.body,
            OperationBody::RestoreFootprint(RestoreFootprintOp {
                ext: ExtensionPoint::V0
            })
        ));

        let xdr = plan.to_xdr_base64().unwrap();
        let decoded = LedgerFootprint::from_xdr_base64(&xdr, Limits::none()).unwrap();
        assert_eq!(decoded, plan.footprint);
    }
}
