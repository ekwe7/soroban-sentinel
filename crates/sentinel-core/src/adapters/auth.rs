//! XDR auth and ledger-footprint parsing adapters.

use stellar_xdr::curr::{LedgerFootprint, LedgerKey, Limits, ReadXdr, VecM, WriteXdr};

use crate::error::{SentinelError, SentinelResult};

/// Parses a base64-encoded Soroban `LedgerFootprint`.
pub fn parse_ledger_footprint(base64: &str) -> SentinelResult<LedgerFootprint> {
    LedgerFootprint::from_xdr_base64(base64, Limits::none()).map_err(|error| {
        SentinelError::InvalidXdrKey(format!("Failed to decode LedgerFootprint: {error}"))
    })
}

/// Encodes a footprint as base64 XDR.
pub fn footprint_to_xdr_base64(footprint: &LedgerFootprint) -> SentinelResult<String> {
    footprint.to_xdr_base64(Limits::none()).map_err(|error| {
        SentinelError::SerializationError(format!("Failed to encode LedgerFootprint: {error}"))
    })
}

/// Returns all read-only and read-write keys in deterministic order.
pub fn split_footprint_keys(footprint: &LedgerFootprint) -> (Vec<LedgerKey>, Vec<LedgerKey>) {
    let mut read_only: Vec<LedgerKey> = footprint.read_only.iter().cloned().collect();
    let mut read_write: Vec<LedgerKey> = footprint.read_write.iter().cloned().collect();
    read_only.sort_unstable();
    read_write.sort_unstable();
    read_only.dedup();
    read_write.dedup();
    (read_only, read_write)
}

/// Constructs a read-only footprint from ledger keys.
pub fn read_only_footprint(keys: &[LedgerKey]) -> SentinelResult<LedgerFootprint> {
    let read_only = VecM::try_from(keys.to_vec()).map_err(|error| {
        SentinelError::InvalidXdrKey(format!("Invalid LedgerFootprint keys: {error}"))
    })?;
    Ok(LedgerFootprint {
        read_only,
        read_write: VecM::default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use stellar_xdr::curr::{
        ContractDataDurability, Hash, LedgerKeyContractData, ScAddress, ScVal,
    };

    fn key(value: u8) -> LedgerKey {
        LedgerKey::ContractData(LedgerKeyContractData {
            contract: ScAddress::Contract(Hash([9; 32])),
            key: ScVal::U32(value as u32),
            durability: ContractDataDurability::Persistent,
        })
    }

    #[test]
    fn round_trips_ledger_footprint() {
        let footprint = read_only_footprint(&[key(1)]).unwrap();
        let encoded = footprint_to_xdr_base64(&footprint).unwrap();
        let parsed = parse_ledger_footprint(&encoded).unwrap();
        assert_eq!(parsed, footprint);
    }

    #[test]
    fn rejects_invalid_footprint_xdr() {
        assert!(parse_ledger_footprint("not-valid-base64!").is_err());
    }
}
