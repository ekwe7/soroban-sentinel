use sentinel_core::{detect_shadowed_keys, Durability, EntryType, KeyCollisionKind, StorageEntry};
use stellar_xdr::curr::{Limits, ScBytes, ScVal, WriteXdr};

fn entry(key: &str, durability: Durability) -> StorageEntry {
    StorageEntry::new(key, EntryType::PersistentData, durability, 64, 100, 90)
}

#[test]
fn clean_keys_do_not_produce_collision_warnings() {
    let entries = [
        entry("persistent:Balance", Durability::Persistent),
        entry("temporary:Session", Durability::Temporary),
    ];
    assert!(detect_shadowed_keys(&entries).is_empty());
}

#[test]
fn identical_scval_keys_across_temporary_and_persistent_storage_are_reported() {
    let entries = [
        entry("temporary:Owner", Durability::Temporary),
        entry("persistent:Owner", Durability::Persistent),
    ];
    let warnings = detect_shadowed_keys(&entries);
    assert_eq!(warnings.len(), 1);
    assert_eq!(warnings[0].kind, KeyCollisionKind::CrossDurability);
    assert_eq!(warnings[0].first_durability, Durability::Temporary);
    assert_eq!(warnings[0].second_durability, Durability::Persistent);
}

#[test]
fn raw_bytes_that_match_a_normalized_symbol_are_reported() {
    let bytes = ScVal::Bytes(ScBytes(b"Balance".to_vec().try_into().unwrap()))
        .to_xdr_base64(Limits::none())
        .unwrap();
    let entries = [
        entry(&format!("temporary_xdr:{bytes}"), Durability::Temporary),
        entry("persistent:Balance", Durability::Persistent),
    ];
    let warnings = detect_shadowed_keys(&entries);
    assert_eq!(warnings.len(), 1);
    assert_eq!(warnings[0].kind, KeyCollisionKind::RawBytesSymbol);
}
