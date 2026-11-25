use sentinel_core::domain::{
    calculate_batch_envelope_size, estimate_entry_footprint_bytes, pack_extension_batches,
    Durability, EntryType, StorageEntry, BASE_ENVELOPE_OVERHEAD_BYTES,
};
use stellar_xdr::curr::{
    ContractDataDurability, Hash, LedgerKey, LedgerKeyContractData, Limits, ScAddress, ScVal,
    WriteXdr,
};

fn sample_entry(key: &str, byte_size: usize) -> StorageEntry {
    StorageEntry::new(
        key,
        EntryType::PersistentData,
        Durability::Persistent,
        byte_size,
        1_000_000,
        900_000,
    )
}

fn sample_xdr_entry(seed: u8) -> StorageEntry {
    let key = LedgerKey::ContractData(LedgerKeyContractData {
        contract: ScAddress::Contract(Hash([seed; 32])),
        key: ScVal::U32(seed as u32),
        durability: ContractDataDurability::Persistent,
    });
    let key_xdr = key.to_xdr_base64(Limits::none()).unwrap();
    StorageEntry::new(
        key_xdr,
        EntryType::PersistentData,
        Durability::Persistent,
        64,
        1_000_000,
        900_000,
    )
}

#[test]
fn test_empty_or_zero_ceiling_returns_empty() {
    let entries = vec![sample_entry("k1", 50)];
    assert!(pack_extension_batches(&[], 1000).is_empty());
    assert!(pack_extension_batches(&entries, 0).is_empty());
}

#[test]
fn test_single_entry_within_limit() {
    let entries = vec![sample_entry("k1", 100)];
    let max_bytes = BASE_ENVELOPE_OVERHEAD_BYTES + 100;
    let batches = pack_extension_batches(&entries, max_bytes);

    assert_eq!(batches.len(), 1);
    assert_eq!(batches[0].len(), 1);
    assert_eq!(batches[0][0].key_identifier(), "k1");
    assert!(calculate_batch_envelope_size(&batches[0]) <= max_bytes);
}

#[test]
fn test_batch_splits_when_exceeding_ceiling() {
    // 4 entries of 100 bytes each. Base overhead is 160.
    // If max_bytes is 300:
    // Batch 1 can hold overhead (160) + entry (100) = 260. A second entry would make 360 > 300.
    // So each entry must get its own batch (4 batches).
    let entries = vec![
        sample_entry("k1", 100),
        sample_entry("k2", 100),
        sample_entry("k3", 100),
        sample_entry("k4", 100),
    ];
    let max_bytes = 300;
    let batches = pack_extension_batches(&entries, max_bytes);

    assert_eq!(batches.len(), 4);
    for batch in &batches {
        assert_eq!(batch.len(), 1);
        assert!(calculate_batch_envelope_size(batch) <= max_bytes);
    }
}

#[test]
fn test_bin_packing_minimizes_total_batches() {
    // Bin capacity: overhead (160) + 140 = 300 bytes available for entries.
    // Items of sizes: 90, 80, 60, 50, 40, 30. Total payload = 350.
    // Best-fit decreasing packing into 300-byte bins (140 payload cap per bin):
    // Bin 1: 90 + 50 = 140 (exact full fit!)
    // Bin 2: 80 + 60 = 140 (exact full fit!)
    // Bin 3: 40 + 30 = 70
    // Total bins: 3.
    let entries = vec![
        sample_entry("e30", 30),
        sample_entry("e50", 50),
        sample_entry("e90", 90),
        sample_entry("e40", 40),
        sample_entry("e80", 80),
        sample_entry("e60", 60),
    ];
    let max_bytes = BASE_ENVELOPE_OVERHEAD_BYTES + 140;
    let batches = pack_extension_batches(&entries, max_bytes);

    assert_eq!(batches.len(), 3);
    for batch in &batches {
        assert!(calculate_batch_envelope_size(batch) <= max_bytes);
    }

    let all_keys: usize = batches.iter().map(|b| b.len()).sum();
    assert_eq!(all_keys, 6);
}

#[test]
fn test_real_xdr_keys_footprint_measurement() {
    let entries = vec![
        sample_xdr_entry(1),
        sample_xdr_entry(2),
        sample_xdr_entry(3),
    ];
    for entry in &entries {
        let size = estimate_entry_footprint_bytes(entry);
        assert!(size > 0);
    }

    let total_size = calculate_batch_envelope_size(&entries);
    assert!(total_size > BASE_ENVELOPE_OVERHEAD_BYTES);

    let batches = pack_extension_batches(&entries, total_size);
    assert_eq!(batches.len(), 1);
    assert_eq!(batches[0].len(), 3);
}
