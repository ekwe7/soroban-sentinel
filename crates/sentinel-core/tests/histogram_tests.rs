use sentinel_core::domain::{Durability, EntryType, ExpirationHistogram, RentRates, StorageEntry};
use std::num::{NonZeroU32, NonZeroU64};

fn rates() -> RentRates {
    RentRates {
        fee_per_rent_kib_stroops: 1024,
        persistent_denominator: NonZeroU64::new(1).unwrap(),
        temporary_denominator: NonZeroU64::new(2).unwrap(),
        ledgers_per_year: NonZeroU32::new(6_307_200).unwrap(),
        code_rent_discount: NonZeroU32::new(3).unwrap(),
    }
}

fn entry(index: u32, remaining: u32, durability: Durability) -> StorageEntry {
    StorageEntry::new(
        format!("contract-{index}:key"),
        EntryType::PersistentData,
        durability,
        10,
        100 + remaining,
        1,
    )
}

#[test]
fn exact_day_boundaries_partition_all_buckets() {
    let day = 17280;
    let horizons = [
        0,
        day - 1,
        day,
        7 * day - 1,
        7 * day,
        30 * day - 1,
        30 * day,
        90 * day - 1,
        90 * day,
    ];
    let entries: Vec<_> = horizons
        .into_iter()
        .enumerate()
        .map(|(index, remaining)| entry(index as u32, remaining, Durability::Persistent))
        .collect();
    let histogram = ExpirationHistogram::aggregate(&entries, 100, 5.0, &rates()).unwrap();
    assert_eq!(histogram.total_count, 9);
    assert_eq!(
        histogram
            .buckets
            .iter()
            .map(|bucket| bucket.count)
            .collect::<Vec<_>>(),
        [2, 2, 2, 2, 1]
    );
    assert_eq!(histogram.buckets[0].total_bytes, 20);
    assert_eq!(
        histogram.buckets[0].projected_rent_stroops,
        u128::from(day - 1) * 10
    );
    assert_eq!(histogram.p10_remaining_ledgers, Some(0));
    assert_eq!(histogram.p50_remaining_ledgers, Some(7 * day as i64));
    assert_eq!(histogram.p90_remaining_ledgers, Some(90 * day as i64));
}

#[test]
fn ledger_velocity_changes_buckets_but_not_rent_or_ledger_percentiles() {
    let entries = [
        entry(1, 10000, Durability::Temporary),
        entry(2, 10000, Durability::Code),
    ];
    let normal = ExpirationHistogram::aggregate(&entries, 100, 5.0, &rates()).unwrap();
    let slow = ExpirationHistogram::aggregate(&entries, 100, 10.0, &rates()).unwrap();
    assert_eq!(normal.buckets[0].count, 2);
    assert_eq!(slow.buckets[1].count, 2);
    assert_eq!(normal.buckets[0].projected_rent_stroops, 50000 + 33334);
    assert_eq!(
        slow.buckets[1].projected_rent_stroops,
        normal.buckets[0].projected_rent_stroops
    );
    assert_eq!(normal.p50_remaining_ledgers, slow.p50_remaining_ledgers);
}

#[test]
fn empty_and_expired_inputs_have_defined_statistics_and_no_negative_rent() {
    let empty = ExpirationHistogram::aggregate(&[], 100, 5.0, &rates()).unwrap();
    assert_eq!(empty.total_count, 0);
    assert_eq!(empty.p10_remaining_ledgers, None);
    let expired = StorageEntry::new(
        "expired",
        EntryType::PersistentData,
        Durability::Persistent,
        20,
        90,
        1,
    );
    let histogram = ExpirationHistogram::aggregate(&[expired], 100, 5.0, &rates()).unwrap();
    assert_eq!(histogram.p90_remaining_ledgers, Some(-10));
    assert_eq!(histogram.buckets[0].projected_rent_stroops, 0);
    assert!(ExpirationHistogram::aggregate(&[], 100, f64::NAN, &rates()).is_err());
}
