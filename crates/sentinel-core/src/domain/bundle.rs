//! Pure planning and XDR construction for batched footprint TTL extensions.

use std::collections::HashSet;

use stellar_xdr::curr::{ExtensionPoint, LedgerFootprint, LedgerKey, Limits, VecM, WriteXdr};
use thiserror::Error;

/// Maximum number of ledger keys allowed in a single bundle transaction.
pub const MAX_BUNDLE_FOOTPRINT_ENTRIES: usize = 40;

/// Errors raised while preparing transaction footprint bundles.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum BundleError {
    /// An empty footprint or invalid per-transaction limit was supplied.
    #[error("Invalid bundle footprint: {0}")]
    InvalidFootprint(String),

    /// XDR footprint encoding failed.
    #[error("Failed to encode bundle footprint: {0}")]
    Xdr(String),
}

/// A read-only footprint prepared for one ExtendFootprintTTL transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundleFootprint {
    /// Deduplicated keys included in this transaction.
    pub keys: Vec<LedgerKey>,
    /// Soroban footprint with the bundled keys in `read_only`.
    pub footprint: LedgerFootprint,
}

impl BundleFootprint {
    /// Encodes the footprint as base64 XDR.
    pub fn to_xdr_base64(&self) -> Result<String, BundleError> {
        self.footprint
            .to_xdr_base64(Limits::none())
            .map_err(|error| BundleError::Xdr(error.to_string()))
    }

    /// The extension operation uses protocol extension point V0.
    pub fn extension_point(&self) -> ExtensionPoint {
        ExtensionPoint::V0
    }
}

/// Deduplicates ledger keys and splits them into read-only footprints.
///
/// Duplicate keys are removed and the remaining keys are sorted for a
/// deterministic footprint. No emitted footprint exceeds the protocol's
/// conservative 40-entry bundle limit.
pub fn plan_bundle_footprints(
    keys: &[LedgerKey],
    max_entries: usize,
) -> Result<Vec<BundleFootprint>, BundleError> {
    if max_entries == 0 || max_entries > MAX_BUNDLE_FOOTPRINT_ENTRIES {
        return Err(BundleError::InvalidFootprint(format!(
            "per-transaction limit must be between 1 and {}",
            MAX_BUNDLE_FOOTPRINT_ENTRIES
        )));
    }
    if keys.is_empty() {
        return Err(BundleError::InvalidFootprint(
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
    let mut bundles = Vec::with_capacity(unique.len().div_ceil(max_entries));

    for chunk in unique.chunks(max_entries) {
        let read_only = VecM::<LedgerKey, { u32::MAX }>::try_from(chunk.to_vec())
            .map_err(|error| BundleError::InvalidFootprint(error.to_string()))?;
        bundles.push(BundleFootprint {
            keys: chunk.to_vec(),
            footprint: LedgerFootprint {
                read_only,
                read_write: VecM::default(),
            },
        });
    }

    Ok(bundles)
}

#[cfg(test)]
mod tests {
    use super::*;
    use stellar_xdr::curr::{
        ContractDataDurability, Hash, LedgerKeyContractCode, LedgerKeyContractData, ScAddress,
        ScVal,
    };

    fn key(value: u8) -> LedgerKey {
        LedgerKey::ContractData(LedgerKeyContractData {
            contract: ScAddress::Contract(Hash([value; 32])),
            key: ScVal::U32(value as u32),
            durability: ContractDataDurability::Persistent,
        })
    }

    #[test]
    fn deduplicates_keys_before_splitting_deterministic_footprints() {
        let keys = [key(1), key(2), key(1), key(3)];
        let bundles = plan_bundle_footprints(&keys, 2).unwrap();
        assert_eq!(bundles.len(), 2);
        assert_eq!(bundles[0].keys, vec![key(1), key(2)]);
        assert_eq!(bundles[1].keys, vec![key(3)]);
        assert!(bundles
            .iter()
            .all(|bundle| bundle.footprint.read_write.is_empty()));
    }

    #[test]
    fn rejects_empty_input_and_limits_above_protocol_cap() {
        assert!(matches!(
            plan_bundle_footprints(&[], MAX_BUNDLE_FOOTPRINT_ENTRIES),
            Err(BundleError::InvalidFootprint(_))
        ));
        let one = [LedgerKey::ContractCode(LedgerKeyContractCode {
            hash: Hash([1; 32]),
        })];
        assert!(matches!(
            plan_bundle_footprints(&one, MAX_BUNDLE_FOOTPRINT_ENTRIES + 1),
            Err(BundleError::InvalidFootprint(_))
        ));
    }
}
