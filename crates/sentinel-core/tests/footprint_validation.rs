//! Integration coverage for invocation footprint TTL validation.

use std::collections::HashMap;

use sentinel_core::domain::{Durability, EntryType};
use sentinel_core::{FootprintAccess, FootprintValidator, StorageEntry};
use stellar_xdr::curr::{
    ContractDataDurability, Hash, LedgerKey, LedgerKeyContractData, ScAddress, ScVal, WriteXdr,
};

fn persistent_key(value: u8) -> LedgerKey {
    LedgerKey::ContractData(LedgerKeyContractData {
        contract: ScAddress::Contract(Hash([21; 32])),
        key: ScVal::U32(value as u32),
        durability: ContractDataDurability::Persistent,
    })
}

fn encoded(key: &LedgerKey) -> String {
    key.to_xdr_base64(stellar_xdr::curr::Limits::none())
        .unwrap()
}

#[test]
fn detects_at_risk_footprint_against_scanned_storage_map() {
    let key = persistent_key(7);
    let key_xdr = encoded(&key);

    let scanned = StorageEntry::new(
        "persistent:7",
        EntryType::PersistentData,
        Durability::Persistent,
        64,
        1_050,
        900,
    );
    let mut entries = HashMap::new();
    entries.insert(key_xdr, scanned);

    let validator = FootprintValidator::taking_keys(&[key], 100);
    let result = validator.validate_storage_entries(1_000, &entries);

    assert!(!result.is_safe());
    assert_eq!(result.at_risk.len(), 1);
    assert_eq!(result.at_risk[0].access, FootprintAccess::ReadOnly);
    assert_eq!(result.at_risk[0].remaining_ledgers, 50);
}

#[test]
fn read_write_footprint_with_safe_ttl_is_valid() {
    use stellar_xdr::curr::{LedgerFootprint, VecM};

    let key = persistent_key(8);
    let key_xdr = encoded(&key);
    let footprint = LedgerFootprint {
        read_only: VecM::default(),
        read_write: VecM::try_from(vec![key]).unwrap(),
    };
    let validator = FootprintValidator::new(footprint, 100);

    let mut ttls = HashMap::new();
    ttls.insert(key_xdr, 1_200);

    assert!(validator.validate_ttl_map(1_000, &ttls).is_safe());
}
