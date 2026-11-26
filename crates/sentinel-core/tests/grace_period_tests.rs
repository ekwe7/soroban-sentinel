use sentinel_core::domain::{
    calculate_grace_window, Durability, EntryType, GracePeriodStatus, NetworkConfig, StorageEntry,
};
use std::num::{NonZeroU32, NonZeroU64};

fn config(window: Option<u32>) -> NetworkConfig {
    NetworkConfig {
        min_persistent_ttl: NonZeroU32::new(100).unwrap(),
        max_entry_ttl: NonZeroU32::new(1000).unwrap(),
        ledger_close_seconds: NonZeroU32::new(5).unwrap(),
        write_fee_per_kib_stroops: 1024,
        write_fee_per_entry_stroops: 10,
        persistent_rent_rate_denominator: NonZeroU64::new(10).unwrap(),
        operator_recovery_window_ledgers: window.and_then(NonZeroU32::new),
    }
}
fn entry(durability: Durability) -> StorageEntry {
    StorageEntry::new("key", EntryType::PersistentData, durability, 100, 1000, 1)
}

#[test]
fn live_until_is_inclusive_and_near_expiry_remains_active() {
    let persistent = entry(Durability::Persistent);
    assert_eq!(
        calculate_grace_window(&persistent, 900, &config(None)),
        GracePeriodStatus::Active {
            ledgers_until_expiry: 100,
            seconds_until_expiry: 500
        }
    );
    assert_eq!(
        calculate_grace_window(&persistent, 1000, &config(None)),
        GracePeriodStatus::Active {
            ledgers_until_expiry: 0,
            seconds_until_expiry: 0
        }
    );
}
#[test]
fn persistent_restoration_is_unbounded_and_arrears_are_explicitly_modeled() {
    let GracePeriodStatus::InGracePeriod {
        operator_deadline_ledger,
        operator_ledgers_remaining,
        operator_deadline_missed,
        restoration_cost,
        ..
    } = calculate_grace_window(&entry(Durability::Persistent), 2000, &config(None))
    else {
        panic!("must remain restorable")
    };
    assert_eq!(operator_deadline_ledger, None);
    assert_eq!(operator_ledgers_remaining, None);
    assert!(!operator_deadline_missed);
    assert_eq!(restoration_cost.arrears_recovery_fee_stroops, 10000);
    assert_eq!(restoration_cost.new_lifetime_extension_fee_stroops, 1000);
    assert_eq!(restoration_cost.ttl_write_fee_stroops, 78);
    assert_eq!(restoration_cost.restoration_rent_stroops, 1078);
}
#[test]
fn operator_deadline_changes_urgency_without_destroying_persistent_data() {
    let persistent = entry(Durability::Persistent);
    let GracePeriodStatus::InGracePeriod {
        operator_ledgers_remaining,
        operator_seconds_remaining,
        ..
    } = calculate_grace_window(&persistent, 1010, &config(Some(100)))
    else {
        panic!("restorable")
    };
    assert_eq!(operator_ledgers_remaining, Some(90));
    assert_eq!(operator_seconds_remaining, Some(450));
    for ledger in [1100, 1200] {
        let GracePeriodStatus::InGracePeriod {
            operator_deadline_missed,
            operator_ledgers_remaining,
            ..
        } = calculate_grace_window(&persistent, ledger, &config(Some(100)))
        else {
            panic!("restorable after deadline")
        };
        assert!(operator_deadline_missed);
        assert_eq!(operator_ledgers_remaining, Some(0));
    }
}
#[test]
fn temporary_data_has_no_recovery_window() {
    let temporary = entry(Durability::Temporary);
    assert!(matches!(
        calculate_grace_window(&temporary, 1000, &config(Some(100))),
        GracePeriodStatus::Active { .. }
    ));
    assert_eq!(
        calculate_grace_window(&temporary, 1001, &config(Some(100))),
        GracePeriodStatus::PermanentlyEvicted
    );
}
#[test]
fn missing_ttl_and_invalid_config_remain_unknown() {
    let missing = StorageEntry::new(
        "missing",
        EntryType::PersistentData,
        Durability::Persistent,
        100,
        0,
        0,
    );
    assert_eq!(
        calculate_grace_window(&missing, 1000, &config(None)),
        GracePeriodStatus::Unknown
    );
    let mut invalid = config(None);
    invalid.min_persistent_ttl = NonZeroU32::new(1001).unwrap();
    assert_eq!(
        calculate_grace_window(&entry(Durability::Persistent), 1, &invalid),
        GracePeriodStatus::Unknown
    );
}
