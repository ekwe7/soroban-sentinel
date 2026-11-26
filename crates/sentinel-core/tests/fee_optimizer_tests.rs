use sentinel_core::{optimize_bump_batch, FeeOptimizerEntry, UrgencyTier};

#[test]
fn classifies_urgency_boundaries_and_keeps_schedule_within_budget() {
    let entries = vec![
        FeeOptimizerEntry {
            key: "immediate-a".into(),
            remaining_days: 6,
            requested_extension_ledgers: 4,
            stroops_per_ledger: 2,
        },
        FeeOptimizerEntry {
            key: "immediate-b".into(),
            remaining_days: 0,
            requested_extension_ledgers: 4,
            stroops_per_ledger: 2,
        },
        FeeOptimizerEntry {
            key: "warning".into(),
            remaining_days: 7,
            requested_extension_ledgers: 4,
            stroops_per_ledger: 2,
        },
        FeeOptimizerEntry {
            key: "stable".into(),
            remaining_days: 30,
            requested_extension_ledgers: 4,
            stroops_per_ledger: 2,
        },
    ];
    let schedule = optimize_bump_batch(&entries, 12);
    assert_eq!(schedule[0].urgency, UrgencyTier::ImmediateCliff);
    assert_eq!(schedule[2].urgency, UrgencyTier::WarningHorizon);
    assert_eq!(schedule[3].urgency, UrgencyTier::Stable);
    assert_eq!(
        schedule
            .iter()
            .map(|item| item.estimated_fee_stroops)
            .sum::<u64>(),
        12
    );
    assert!(
        schedule[0].suggested_extension_ledgers > 0 && schedule[1].suggested_extension_ledgers > 0
    );
    assert_eq!(schedule[2].suggested_extension_ledgers, 0);
    assert_eq!(schedule[3].suggested_extension_ledgers, 0);
}

#[test]
fn lower_urgency_is_considered_only_after_higher_tier_is_fully_extended() {
    let entries = vec![
        FeeOptimizerEntry {
            key: "urgent".into(),
            remaining_days: 1,
            requested_extension_ledgers: 2,
            stroops_per_ledger: 1,
        },
        FeeOptimizerEntry {
            key: "stable".into(),
            remaining_days: 31,
            requested_extension_ledgers: 2,
            stroops_per_ledger: 1,
        },
    ];
    let schedule = optimize_bump_batch(&entries, 3);
    assert_eq!(schedule[0].suggested_extension_ledgers, 2);
    assert_eq!(schedule[1].suggested_extension_ledgers, 1);
}

#[test]
fn large_requests_are_allocated_without_per_ledger_iteration() {
    let entries = vec![FeeOptimizerEntry {
        key: "urgent".into(),
        remaining_days: 1,
        requested_extension_ledgers: u32::MAX,
        stroops_per_ledger: 1,
    }];
    let schedule = optimize_bump_batch(&entries, u64::MAX);
    assert_eq!(schedule[0].suggested_extension_ledgers, u32::MAX);
    assert_eq!(schedule[0].estimated_fee_stroops, u64::from(u32::MAX));
}
