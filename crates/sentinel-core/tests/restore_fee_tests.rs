use sentinel_core::domain::{
    Durability, EntryType, RestoreFeeCalculator, RestoreFeeError, StorageEntry,
};

fn calculator() -> RestoreFeeCalculator {
    // Initial SorobanNetworkConfig from stellar-core v21.0.0, using the
    // protocol's effective write fee floor as the bucket-list rate fixture.
    RestoreFeeCalculator {
        fee_write_1kb: 1_000,
        min_persistent_entry_ttl: 4_096.try_into().unwrap(),
        fee_write_entry: 20_000,
        fee_read_1kb: 1_000,
        fee_read_entry: 5_000,
        persistent_rent_rate_denominator: 252_480.try_into().unwrap(),
    }
}

fn entry(key: &str, bytes: usize, durability: Durability) -> StorageEntry {
    StorageEntry::new(key, EntryType::PersistentData, durability, bytes, 100, 50)
}

#[test]
fn quotes_exact_stroops_and_xlm_for_a_kibibyte() {
    let calc = calculator();
    let archived = entry("key", 1_024, Durability::Persistent);
    let fee = calc.calculate(&archived).unwrap();
    assert_eq!(fee.base_restoration_fee_stroops, 47_047);
    assert_eq!(fee.initial_rent_stroops, 17);
    assert_eq!(fee.total_stroops, 47_064);
    assert_eq!(fee.total_xlm, "0.0047064");

    let longer_ttl = RestoreFeeCalculator {
        min_persistent_entry_ttl: 8_192.try_into().unwrap(),
        ..calc
    };
    assert_eq!(
        longer_ttl
            .calculate(&archived)
            .unwrap()
            .initial_rent_stroops,
        33
    );
}

#[test]
fn batch_rounds_rent_per_entry_and_footprint_byte_fees_once() {
    let calc = calculator();
    let first = entry("one", 24, Durability::Persistent);
    let second = entry("two", 24, Durability::Persistent);
    let fee = calc.calculate_batch(&[first, second]).unwrap();
    assert_eq!(fee.base_restoration_fee_stroops, 90_188);
    assert_eq!(fee.initial_rent_stroops, 2);
    assert_eq!(fee.total_xlm, "0.0090190");
    assert_eq!(calc.calculate_batch(&[]).unwrap().total_stroops, 0);
}

#[test]
fn rejects_temporary_entries_and_unknown_sizes() {
    let calc = calculator();
    assert_eq!(
        calc.calculate(&entry("temp", 24, Durability::Temporary)),
        Err(RestoreFeeError::TemporaryEntry)
    );
    assert_eq!(
        calc.calculate(&entry("unknown", 0, Durability::Persistent)),
        Err(RestoreFeeError::MissingSize)
    );
}
