use sentinel_core::domain::{
    calculate_storage_overhead, measure_contract_data_overhead, Durability, EntryType, StorageEntry,
};
use stellar_xdr::curr::{BytesM, ScBytes, ScSymbol, ScVal, StringM};

fn test_entry(key: &str, billable_bytes: usize, payload_bytes: usize) -> StorageEntry {
    StorageEntry::new(
        key,
        EntryType::PersistentData,
        Durability::Persistent,
        billable_bytes,
        2_000_000,
        1_900_000,
    )
    .with_payload_bytes(payload_bytes)
}

#[test]
fn test_tiny_integer_key_overhead_exceeds_60_percent() {
    // A tiny integer key/value: 68 bytes total LedgerEntry, only 8 bytes raw payload.
    // Overhead = 60 bytes (88.23%).
    let entry = test_entry("counter_key", 68, 8);
    let analysis = calculate_storage_overhead(&entry);

    assert_eq!(analysis.billable_bytes, 68);
    assert_eq!(analysis.payload_bytes, 8);
    assert_eq!(analysis.overhead_bytes, 60);

    let expected_ratio = (60.0 / 68.0) * 100.0;
    assert!((analysis.overhead_ratio_percent - expected_ratio).abs() < 1e-6);
    assert!(analysis.overhead_ratio_percent > 60.0);

    // Verifies actionable recommendation is provided for high overhead
    let rec = analysis
        .recommendation
        .expect("expected recommendation for high overhead");
    assert!(rec.contains(">60%"));
}

#[test]
fn test_large_payload_has_low_overhead() {
    // 1024 bytes total, 960 bytes payload => 64 bytes overhead (6.25%).
    let entry = test_entry("blob_key", 1024, 960);
    let analysis = calculate_storage_overhead(&entry);

    assert_eq!(analysis.billable_bytes, 1024);
    assert_eq!(analysis.payload_bytes, 960);
    assert_eq!(analysis.overhead_bytes, 64);

    let expected_ratio = (64.0 / 1024.0) * 100.0;
    assert!((analysis.overhead_ratio_percent - expected_ratio).abs() < 1e-6);
    assert!(analysis.overhead_ratio_percent <= 40.0);
    assert!(analysis.recommendation.is_none());
}

#[test]
fn test_moderate_overhead_recommendation() {
    // 100 bytes total, 50 bytes payload => 50 bytes overhead (50%).
    let entry = test_entry("medium_key", 100, 50);
    let analysis = calculate_storage_overhead(&entry);

    assert_eq!(analysis.overhead_ratio_percent, 50.0);
    let rec = analysis
        .recommendation
        .expect("expected recommendation for moderate overhead");
    assert!(rec.contains(">40%"));
}

#[test]
fn test_measure_exact_xdr_contract_data_overhead() {
    let contract_id = [7u8; 32];
    let key = ScVal::Symbol(ScSymbol(StringM::try_from("COUNT").unwrap()));
    let val = ScVal::U32(42);

    let analysis =
        measure_contract_data_overhead(contract_id, key, val, Durability::Persistent, 500)
            .expect("serialization should succeed");

    // ScVal::U32 is 8 bytes in XDR (4-byte discriminant + 4-byte u32)
    assert_eq!(analysis.payload_bytes, 8);
    // Billable LedgerEntry includes contract address, durability, key, val, ledger seq, and ext
    assert!(analysis.billable_bytes > 50);
    assert!(analysis.overhead_bytes > 0);
    assert!(analysis.overhead_ratio_percent > 60.0);
    assert!(analysis.recommendation.is_some());

    // Now test with larger payload
    let large_val = ScVal::Bytes(ScBytes(BytesM::try_from(vec![1u8; 500]).unwrap()));
    let key2 = ScVal::Symbol(ScSymbol(StringM::try_from("DATA").unwrap()));
    let analysis2 =
        measure_contract_data_overhead(contract_id, key2, large_val, Durability::Persistent, 500)
            .expect("serialization should succeed");

    assert_eq!(analysis2.payload_bytes, 508); // 4 discriminant + 4 len + 500 bytes
    assert!(analysis2.overhead_ratio_percent < 20.0);
    assert!(analysis2.recommendation.is_none());
}
