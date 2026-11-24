//! Tests for the rent burn acceleration detector (`StorageAccelerationTracker`).

use sentinel_core::{
    AccelerationError, DecayClassification, Durability, EntryType, RiskLevel, StateGrowth,
    StorageAccelerationTracker, StorageEntryReport, TTLReport,
};

const CONTRACT: &str = "CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC";

fn entry(id: &str, bytes: usize, live_until: u32, remaining: i64) -> StorageEntryReport {
    StorageEntryReport::new(
        id,
        EntryType::PersistentData,
        Durability::Persistent,
        bytes,
        live_until,
        remaining,
        remaining as f64 * 5.0,
        remaining as f64 * 5.0 / 86_400.0,
        RiskLevel::Ok,
    )
}

fn report(ledger: u32, entries: Vec<StorageEntryReport>) -> TTLReport {
    TTLReport {
        contract_id: CONTRACT.to_string(),
        network: "testnet".to_string(),
        latest_ledger: ledger,
        ledger_velocity_seconds: 5.0,
        burn_rate: Default::default(),
        entries,
        overall_status: RiskLevel::Ok,
        archival_recommendations: Vec::new(),
        sac_asset_info: None,
        contract_code_hash: None,
        contract_bytecode_size: None,
        contract_upgrade: None,
    }
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

#[test]
fn steady_state_decay_is_not_flagged() {
    let baseline = report(1_000, vec![entry("k", 100, 101_000, 100_000)]);
    let current = report(1_100, vec![entry("k", 100, 101_000, 99_900)]);

    let result = StorageAccelerationTracker::default()
        .compare(&baseline, &current)
        .expect("compare");

    assert_eq!(result.ledgers_elapsed, 100);
    assert!(close(result.elapsed_seconds, 500.0));
    assert!(!result.has_accelerated_decay());
    assert!(result.warnings.is_empty());

    let e = &result.entries[0];
    assert_eq!(e.decay, DecayClassification::Steady);
    assert_eq!(e.growth, StateGrowth::Unchanged);
    assert_eq!(e.observed_decay_ledgers, 100);
    assert_eq!(e.expected_decay_ledgers, 100);
    assert_eq!(e.excess_decay_ledgers, 0);
    assert!(close(e.byte_expansion_rate, 0.0));
    assert!(close(e.decay_velocity, -0.2));
    assert!(close(e.decay_velocity_delta, 0.0));
}

#[test]
fn expanded_state_growth_flags_accelerated_decay() {
    let baseline = report(1_000, vec![entry("k", 100, 101_000, 100_000)]);
    let current = report(1_100, vec![entry("k", 300, 101_000, 99_500)]);

    let result = StorageAccelerationTracker::default()
        .compare(&baseline, &current)
        .expect("compare");

    let e = &result.entries[0];
    assert_eq!(e.decay, DecayClassification::Accelerated);
    assert_eq!(e.growth, StateGrowth::Expanded);
    assert_eq!(e.byte_delta, 200);
    assert!(close(e.byte_expansion_rate, 2.0));
    assert_eq!(e.observed_decay_ledgers, 500);
    assert_eq!(e.excess_decay_ledgers, 400);
    assert!(close(e.decay_velocity, -1.0));
    assert!(close(e.decay_velocity_delta, -0.8));

    assert!(result.has_accelerated_decay());
    assert_eq!(result.warnings.len(), 1);
    let w = &result.warnings[0];
    assert_eq!(w.identifier, "k");
    assert_eq!(w.expected_decay_ledgers, 100);
    assert_eq!(w.observed_decay_ledgers, 500);
    assert_eq!(w.excess_decay_ledgers, 400);
    assert!(close(w.byte_expansion_rate, 2.0));
}

#[test]
fn reduced_state_shrinkage_is_reported_without_warning() {
    let baseline = report(1_000, vec![entry("k", 300, 101_000, 100_000)]);
    let current = report(1_100, vec![entry("k", 100, 101_000, 99_900)]);

    let result = StorageAccelerationTracker::default()
        .compare(&baseline, &current)
        .expect("compare");

    let e = &result.entries[0];
    assert_eq!(e.decay, DecayClassification::Steady);
    assert_eq!(e.growth, StateGrowth::Shrunk);
    assert_eq!(e.byte_delta, -200);
    assert!(close(e.byte_expansion_rate, -2.0));
    assert!(result.warnings.is_empty());
    assert_eq!(result.total_byte_delta, -200);
    assert!(close(result.aggregate_byte_expansion_rate, -2.0));
}

#[test]
fn extension_is_classified_separately_and_never_flagged() {
    let baseline = report(1_000, vec![entry("k", 100, 101_000, 100_000)]);
    let current = report(1_100, vec![entry("k", 100, 201_000, 199_900)]);

    let result = StorageAccelerationTracker::default()
        .compare(&baseline, &current)
        .expect("compare");

    assert_eq!(result.entries[0].decay, DecayClassification::Extended);
    assert!(result.warnings.is_empty());
}

#[test]
fn decay_within_tolerance_is_steady_and_beyond_is_accelerated() {
    // 100 ledgers elapsed, default tolerance 10% => 10 ledgers of slack.
    let baseline = report(1_000, vec![entry("k", 100, 101_000, 100_000)]);
    let within = report(1_100, vec![entry("k", 100, 101_000, 99_891)]); // lost 109
    let beyond = report(1_100, vec![entry("k", 100, 101_000, 99_889)]); // lost 111

    let tracker = StorageAccelerationTracker::default();
    let a = tracker.compare(&baseline, &within).expect("within");
    let b = tracker.compare(&baseline, &beyond).expect("beyond");

    assert_eq!(a.entries[0].excess_decay_ledgers, 9);
    assert_eq!(a.entries[0].decay, DecayClassification::Steady);
    assert_eq!(b.entries[0].excess_decay_ledgers, 11);
    assert_eq!(b.entries[0].decay, DecayClassification::Accelerated);
}

#[test]
fn zero_tolerance_flags_any_excess_decay() {
    let baseline = report(1_000, vec![entry("k", 100, 101_000, 100_000)]);
    let current = report(1_100, vec![entry("k", 100, 101_000, 99_899)]); // lost 101

    let tracker = StorageAccelerationTracker::new(0.0).expect("tolerance");
    let result = tracker.compare(&baseline, &current).expect("compare");

    assert_eq!(result.entries[0].excess_decay_ledgers, 1);
    assert_eq!(result.entries[0].decay, DecayClassification::Accelerated);
    assert!(close(result.tolerance_ratio, 0.0));
}

#[test]
fn added_and_removed_entries_feed_the_aggregate_rate() {
    let baseline = report(
        1_000,
        vec![
            entry("a", 100, 101_000, 100_000),
            entry("b", 50, 101_000, 100_000),
        ],
    );
    let current = report(
        1_100,
        vec![
            entry("a", 100, 101_000, 99_900),
            entry("c", 400, 101_000, 99_900),
        ],
    );

    let result = StorageAccelerationTracker::default()
        .compare(&baseline, &current)
        .expect("compare");

    assert_eq!(result.added_entries, vec!["c".to_string()]);
    assert_eq!(result.removed_entries, vec!["b".to_string()]);
    assert_eq!(result.entries.len(), 1);
    assert_eq!(result.total_byte_delta, 350);
    assert!(close(result.aggregate_byte_expansion_rate, 3.5));
}

#[test]
fn rejects_contract_and_network_mismatch() {
    let baseline = report(1_000, vec![]);
    let tracker = StorageAccelerationTracker::default();

    let mut other_contract = report(1_100, vec![]);
    other_contract.contract_id = "CAAA".to_string();
    assert!(matches!(
        tracker.compare(&baseline, &other_contract),
        Err(AccelerationError::ContractMismatch { .. })
    ));

    let mut other_network = report(1_100, vec![]);
    other_network.network = "mainnet".to_string();
    assert!(matches!(
        tracker.compare(&baseline, &other_network),
        Err(AccelerationError::NetworkMismatch { .. })
    ));
}

#[test]
fn rejects_non_advancing_ledgers() {
    let tracker = StorageAccelerationTracker::default();
    let baseline = report(1_000, vec![]);

    assert!(matches!(
        tracker.compare(&baseline, &report(1_000, vec![])),
        Err(AccelerationError::NonAdvancingLedgers { .. })
    ));
    assert!(matches!(
        tracker.compare(&baseline, &report(900, vec![])),
        Err(AccelerationError::NonAdvancingLedgers { .. })
    ));
}

#[test]
fn rejects_invalid_tolerance() {
    assert!(matches!(
        StorageAccelerationTracker::new(-0.1),
        Err(AccelerationError::InvalidTolerance(_))
    ));
    assert!(matches!(
        StorageAccelerationTracker::new(f64::NAN),
        Err(AccelerationError::InvalidTolerance(_))
    ));
}
