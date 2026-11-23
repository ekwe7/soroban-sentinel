//! Pure integer allocation for TTL extension batches under a stroop budget.

/// Urgency derives from remaining wall-clock days before expiration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum UrgencyTier {
    ImmediateCliff,
    WarningHorizon,
    Stable,
}

impl UrgencyTier {
    pub fn from_remaining_days(days: u32) -> Self {
        if days < 7 {
            Self::ImmediateCliff
        } else if days < 30 {
            Self::WarningHorizon
        } else {
            Self::Stable
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeeOptimizerEntry {
    pub key: String,
    pub remaining_days: u32,
    pub requested_extension_ledgers: u32,
    pub stroops_per_ledger: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduledExtension {
    pub key: String,
    pub urgency: UrgencyTier,
    pub requested_extension_ledgers: u32,
    pub suggested_extension_ledgers: u32,
    pub estimated_fee_stroops: u64,
}

/// Spend the budget in urgency order, allocating proportional ledger shares within
/// each tier. Stable entries cannot receive an extension until all affordable
/// ImmediateCliff and WarningHorizon requests have been considered.
pub fn optimize_bump_batch(
    entries: &[FeeOptimizerEntry],
    total_budget_stroops: u64,
) -> Vec<ScheduledExtension> {
    let mut scheduled: Vec<_> = entries
        .iter()
        .map(|entry| ScheduledExtension {
            key: entry.key.clone(),
            urgency: UrgencyTier::from_remaining_days(entry.remaining_days),
            requested_extension_ledgers: entry.requested_extension_ledgers,
            suggested_extension_ledgers: 0,
            estimated_fee_stroops: 0,
        })
        .collect();
    let mut remaining = u128::from(total_budget_stroops);
    for tier in [
        UrgencyTier::ImmediateCliff,
        UrgencyTier::WarningHorizon,
        UrgencyTier::Stable,
    ] {
        let mut indexes: Vec<usize> = scheduled
            .iter()
            .enumerate()
            .filter_map(|(i, item)| (item.urgency == tier).then_some(i))
            .collect();
        // Within a tier distribute the available units round-robin, so large entries
        // do not consume the whole budget before peers receive their first extension.
        indexes.sort_by(|a, b| scheduled[*a].key.cmp(&scheduled[*b].key));
        for index in &indexes {
            if entries[*index].stroops_per_ledger == 0 {
                scheduled[*index].suggested_extension_ledgers =
                    scheduled[*index].requested_extension_ledgers;
            }
        }
        while remaining > 0 {
            let affordable: Vec<_> = indexes
                .iter()
                .copied()
                .filter(|index| {
                    scheduled[*index].suggested_extension_ledgers
                        < scheduled[*index].requested_extension_ledgers
                        && u128::from(entries[*index].stroops_per_ledger) <= remaining
                })
                .collect();
            if affordable.is_empty() {
                break;
            }
            let round_cost: u128 = affordable
                .iter()
                .map(|index| u128::from(entries[*index].stroops_per_ledger))
                .sum();
            let max_rounds = affordable
                .iter()
                .map(|index| {
                    scheduled[*index].requested_extension_ledgers
                        - scheduled[*index].suggested_extension_ledgers
                })
                .min()
                .unwrap_or(0);
            let full_rounds = (remaining / round_cost).min(u128::from(max_rounds));
            if full_rounds > 0 {
                for index in affordable {
                    let units = u32::try_from(full_rounds).expect("round count is bounded by u32");
                    let fee =
                        u64::try_from(u128::from(entries[index].stroops_per_ledger) * full_rounds)
                            .expect("allocated fee cannot exceed the u64 budget");
                    scheduled[index].suggested_extension_ledgers += units;
                    scheduled[index].estimated_fee_stroops += fee;
                    remaining -= u128::from(fee);
                }
                continue;
            }

            let mut progressed = false;
            for index in affordable {
                let cost = u128::from(entries[index].stroops_per_ledger);
                if cost > remaining {
                    continue;
                }
                scheduled[index].suggested_extension_ledgers += 1;
                scheduled[index].estimated_fee_stroops += u64::try_from(cost).unwrap();
                remaining -= cost;
                progressed = true;
            }
            if !progressed {
                break;
            }
        }
    }
    scheduled
}
