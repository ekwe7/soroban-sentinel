//! Deterministic Soroban storage TTL extension planner with budget and capacity constraints.

use serde::{Deserialize, Serialize};

use crate::domain::{Durability, EntryType, RiskLevel};
use crate::usecases::scan::StorageEntryReport;

/// Reason why an entry was deferred rather than selected for extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeferralReason {
    /// Total cost would exceed the specified `max_budget_stroops`.
    BudgetExceeded,
    /// Maximum allowed operations in this batch (`max_entries`) has been reached.
    MaxEntriesExceeded,
    /// Entry is not at risk and requires no immediate extension.
    NotAtRisk,
}

impl std::fmt::Display for DeferralReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BudgetExceeded => write!(f, "BudgetExceeded"),
            Self::MaxEntriesExceeded => write!(f, "MaxEntriesExceeded"),
            Self::NotAtRisk => write!(f, "NotAtRisk"),
        }
    }
}

/// Request parameters for generating a prioritized TTL extension plan.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExtensionPlanRequest {
    /// Evaluated storage entry reports to consider for extension.
    pub entries: Vec<StorageEntryReport>,
    /// Target number of ledgers to extend entries by.
    pub target_extension_ledgers: u32,
    /// Optional upper bound on total extension budget in stroops (1 XLM = 10,000,000 stroops).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub max_budget_stroops: Option<i64>,
    /// Optional limit on the number of entries to extend in a single transaction/batch.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub max_entries: Option<usize>,
    /// Default estimated cost per entry in stroops when no dynamic cost function is provided.
    #[serde(default)]
    pub default_entry_cost_stroops: i64,
}

impl ExtensionPlanRequest {
    /// Creates a new `ExtensionPlanRequest`.
    pub fn new(entries: Vec<StorageEntryReport>, target_extension_ledgers: u32) -> Self {
        Self {
            entries,
            target_extension_ledgers,
            max_budget_stroops: None,
            max_entries: None,
            default_entry_cost_stroops: 50_000, // 0.005 XLM default per entry
        }
    }

    /// Sets the maximum budget constraint in stroops.
    pub fn with_max_budget_stroops(mut self, max_budget_stroops: i64) -> Self {
        self.max_budget_stroops = Some(max_budget_stroops);
        self
    }

    /// Sets the maximum entries limit.
    pub fn with_max_entries(mut self, max_entries: usize) -> Self {
        self.max_entries = Some(max_entries);
        self
    }

    /// Sets the default fallback entry cost in stroops.
    pub fn with_default_entry_cost_stroops(mut self, cost_stroops: i64) -> Self {
        self.default_entry_cost_stroops = cost_stroops;
        self
    }
}

/// A storage entry selected for TTL extension in the generated plan.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlannedExtension {
    /// Identifier of the entry (e.g. "instance", "code_wasm", "persistent:...").
    pub key_identifier: String,
    /// Classification of entry type.
    pub entry_type: EntryType,
    /// Storage durability tier.
    pub durability: Durability,
    /// Risk status prior to extension.
    pub status: RiskLevel,
    /// Remaining ledgers prior to extension.
    pub remaining_ledgers: i64,
    /// Estimated cost in stroops to extend this entry.
    pub estimated_cost_stroops: i64,
    /// Projected live-until ledger after extension.
    pub target_live_until: u32,
}

/// A storage entry deferred from extension in the generated plan.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeferredExtension {
    /// Identifier of the deferred entry.
    pub key_identifier: String,
    /// Classification of entry type.
    pub entry_type: EntryType,
    /// Storage durability tier.
    pub durability: Durability,
    /// Risk status prior to extension.
    pub status: RiskLevel,
    /// Remaining ledgers prior to extension.
    pub remaining_ledgers: i64,
    /// Estimated cost in stroops that would have been required.
    pub estimated_cost_stroops: i64,
    /// Reason why the entry was deferred.
    pub reason: DeferralReason,
}

/// Budget-constrained, prioritized plan for Soroban storage TTL extensions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExtensionPlan {
    /// Selected entries to extend, ordered by priority.
    pub selected: Vec<PlannedExtension>,
    /// Deferred entries that could not be scheduled, with explanations.
    pub deferred: Vec<DeferredExtension>,
    /// Aggregate cost in stroops for all selected extensions.
    pub total_cost_stroops: i64,
    /// Remaining unused budget in stroops, if a budget limit was specified.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub remaining_budget_stroops: Option<i64>,
}

/// Priority rank for risk urgency (Archived > Critical > Warn > Ok).
fn risk_priority_rank(status: RiskLevel) -> u8 {
    match status {
        RiskLevel::Archived => 0,
        RiskLevel::Critical => 1,
        RiskLevel::Warn => 2,
        RiskLevel::Ok => 3,
    }
}

/// Priority rank for durability tier: Code & Instance before PersistentData.
fn durability_priority_rank(entry_type: EntryType, durability: Durability) -> u8 {
    match (entry_type, durability) {
        (EntryType::ContractCode, _) | (_, Durability::Code) => 0,
        (EntryType::ContractInstance, _) | (_, Durability::Instance) => 1,
        _ => 2,
    }
}

/// Plans TTL extensions using a pure deterministic priority algorithm.
///
/// Ranking rules:
/// 1. Urgency: `Archived` > `Critical` > `Warn` > `Ok`
/// 2. Durability tie-breaking: `ContractCode` / `ContractInstance` before `PersistentData`
/// 3. Remaining TTL tie-breaking: lowest `remaining_ledgers` first
/// 4. Stable tie-breaking: `key_identifier` ascending
pub fn plan_extension(request: &ExtensionPlanRequest) -> ExtensionPlan {
    plan_extension_with_cost(request, |entry, _target| {
        // Base estimation: default_entry_cost_stroops + proportional size factor if available
        let size_cost = (entry.byte_size as i64) / 10;
        request.default_entry_cost_stroops + size_cost
    })
}

/// Plans TTL extensions using a custom cost estimator callback.
pub fn plan_extension_with_cost<F>(request: &ExtensionPlanRequest, mut cost_fn: F) -> ExtensionPlan
where
    F: FnMut(&StorageEntryReport, u32) -> i64,
{
    if request.entries.is_empty() {
        return ExtensionPlan {
            selected: Vec::new(),
            deferred: Vec::new(),
            total_cost_stroops: 0,
            remaining_budget_stroops: request.max_budget_stroops,
        };
    }

    // Sort entries deterministically by priority
    let mut sorted_entries = request.entries.clone();
    sorted_entries.sort_by(|a, b| {
        risk_priority_rank(a.status)
            .cmp(&risk_priority_rank(b.status))
            .then_with(|| {
                durability_priority_rank(a.entry_type, a.durability)
                    .cmp(&durability_priority_rank(b.entry_type, b.durability))
            })
            .then_with(|| a.remaining_ledgers.cmp(&b.remaining_ledgers))
            .then_with(|| a.key_identifier.cmp(&b.key_identifier))
    });

    let mut selected = Vec::new();
    let mut deferred = Vec::new();
    let mut current_total_cost: i64 = 0;
    let mut remaining_budget = request.max_budget_stroops;

    for entry in &sorted_entries {
        let cost = cost_fn(entry, request.target_extension_ledgers).max(0);

        // Check max_entries constraint
        if let Some(max_entries) = request.max_entries {
            if selected.len() >= max_entries {
                deferred.push(DeferredExtension {
                    key_identifier: entry.key_identifier.clone(),
                    entry_type: entry.entry_type,
                    durability: entry.durability,
                    status: entry.status,
                    remaining_ledgers: entry.remaining_ledgers,
                    estimated_cost_stroops: cost,
                    reason: DeferralReason::MaxEntriesExceeded,
                });
                continue;
            }
        }

        // Check max_budget_stroops constraint
        if let Some(budget) = request.max_budget_stroops {
            if current_total_cost.saturating_add(cost) > budget {
                deferred.push(DeferredExtension {
                    key_identifier: entry.key_identifier.clone(),
                    entry_type: entry.entry_type,
                    durability: entry.durability,
                    status: entry.status,
                    remaining_ledgers: entry.remaining_ledgers,
                    estimated_cost_stroops: cost,
                    reason: DeferralReason::BudgetExceeded,
                });
                continue;
            }
        }

        // Selected for extension
        current_total_cost = current_total_cost.saturating_add(cost);
        if let Some(ref mut rem) = remaining_budget {
            *rem = rem.saturating_sub(cost);
        }

        let target_live_until = entry
            .live_until_ledger
            .saturating_add(request.target_extension_ledgers);

        selected.push(PlannedExtension {
            key_identifier: entry.key_identifier.clone(),
            entry_type: entry.entry_type,
            durability: entry.durability,
            status: entry.status,
            remaining_ledgers: entry.remaining_ledgers,
            estimated_cost_stroops: cost,
            target_live_until,
        });
    }

    ExtensionPlan {
        selected,
        deferred,
        total_cost_stroops: current_total_cost,
        remaining_budget_stroops: remaining_budget,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_report(
        id: &str,
        entry_type: EntryType,
        durability: Durability,
        remaining_ledgers: i64,
        status: RiskLevel,
    ) -> StorageEntryReport {
        StorageEntryReport::new(
            id,
            entry_type,
            durability,
            100,
            5_500_000,
            remaining_ledgers,
            500_000.0,
            5.78,
            status,
        )
    }

    #[test]
    fn test_empty_reports_returns_empty_plan() {
        let request = ExtensionPlanRequest::new(vec![], 100_000).with_max_budget_stroops(1_000_000);
        let plan = plan_extension(&request);

        assert!(plan.selected.is_empty());
        assert!(plan.deferred.is_empty());
        assert_eq!(plan.total_cost_stroops, 0);
        assert_eq!(plan.remaining_budget_stroops, Some(1_000_000));
    }

    #[test]
    fn test_zero_budget_defers_all_entries() {
        let entries = vec![
            make_report(
                "instance",
                EntryType::ContractInstance,
                Durability::Instance,
                100,
                RiskLevel::Critical,
            ),
            make_report(
                "code",
                EntryType::ContractCode,
                Durability::Code,
                100,
                RiskLevel::Critical,
            ),
        ];
        let request = ExtensionPlanRequest::new(entries, 100_000).with_max_budget_stroops(0);
        let plan = plan_extension(&request);

        assert!(plan.selected.is_empty());
        assert_eq!(plan.deferred.len(), 2);
        assert_eq!(plan.deferred[0].reason, DeferralReason::BudgetExceeded);
        assert_eq!(plan.deferred[1].reason, DeferralReason::BudgetExceeded);
        assert_eq!(plan.total_cost_stroops, 0);
        assert_eq!(plan.remaining_budget_stroops, Some(0));
    }

    #[test]
    fn test_prioritization_and_tie_breaking() {
        let entries = vec![
            make_report(
                "data_warn",
                EntryType::PersistentData,
                Durability::Persistent,
                50_000,
                RiskLevel::Warn,
            ),
            make_report(
                "data_critical",
                EntryType::PersistentData,
                Durability::Persistent,
                10_000,
                RiskLevel::Critical,
            ),
            make_report(
                "code_critical",
                EntryType::ContractCode,
                Durability::Code,
                10_000,
                RiskLevel::Critical,
            ),
            make_report(
                "instance_critical",
                EntryType::ContractInstance,
                Durability::Instance,
                10_000,
                RiskLevel::Critical,
            ),
            make_report(
                "archived_entry",
                EntryType::PersistentData,
                Durability::Persistent,
                -5,
                RiskLevel::Archived,
            ),
        ];

        let request = ExtensionPlanRequest::new(entries, 50_000);
        let plan = plan_extension(&request);

        assert_eq!(plan.selected.len(), 5);
        assert!(plan.deferred.is_empty());

        // 1. Archived entry first
        assert_eq!(plan.selected[0].key_identifier, "archived_entry");
        // 2. Critical Code before Instance and Data
        assert_eq!(plan.selected[1].key_identifier, "code_critical");
        // 3. Critical Instance before Data
        assert_eq!(plan.selected[2].key_identifier, "instance_critical");
        // 4. Critical Data before Warn Data
        assert_eq!(plan.selected[3].key_identifier, "data_critical");
        // 5. Warn Data last
        assert_eq!(plan.selected[4].key_identifier, "data_warn");
    }

    #[test]
    fn test_budget_constraint_over_budget_deferrals() {
        let entries = vec![
            make_report(
                "entry_1",
                EntryType::PersistentData,
                Durability::Persistent,
                100,
                RiskLevel::Critical,
            ),
            make_report(
                "entry_2",
                EntryType::PersistentData,
                Durability::Persistent,
                200,
                RiskLevel::Critical,
            ),
            make_report(
                "entry_3",
                EntryType::PersistentData,
                Durability::Persistent,
                300,
                RiskLevel::Critical,
            ),
        ];

        // Cost per entry will be default_entry_cost (50_000) + 10 = 50_010.
        // Budget = 100_050 allows exactly 2 entries.
        let request = ExtensionPlanRequest::new(entries, 50_000).with_max_budget_stroops(100_050);
        let plan = plan_extension(&request);

        assert_eq!(plan.selected.len(), 2);
        assert_eq!(plan.selected[0].key_identifier, "entry_1");
        assert_eq!(plan.selected[1].key_identifier, "entry_2");

        assert_eq!(plan.deferred.len(), 1);
        assert_eq!(plan.deferred[0].key_identifier, "entry_3");
        assert_eq!(plan.deferred[0].reason, DeferralReason::BudgetExceeded);
    }

    #[test]
    fn test_max_entries_constraint() {
        let entries = vec![
            make_report(
                "entry_1",
                EntryType::PersistentData,
                Durability::Persistent,
                100,
                RiskLevel::Critical,
            ),
            make_report(
                "entry_2",
                EntryType::PersistentData,
                Durability::Persistent,
                200,
                RiskLevel::Critical,
            ),
            make_report(
                "entry_3",
                EntryType::PersistentData,
                Durability::Persistent,
                300,
                RiskLevel::Critical,
            ),
        ];

        let request = ExtensionPlanRequest::new(entries, 50_000).with_max_entries(1);
        let plan = plan_extension(&request);

        assert_eq!(plan.selected.len(), 1);
        assert_eq!(plan.selected[0].key_identifier, "entry_1");

        assert_eq!(plan.deferred.len(), 2);
        assert_eq!(plan.deferred[0].reason, DeferralReason::MaxEntriesExceeded);
        assert_eq!(plan.deferred[1].reason, DeferralReason::MaxEntriesExceeded);
    }

    #[test]
    fn test_custom_cost_estimator_callback() {
        let entries = vec![make_report(
            "entry_1",
            EntryType::PersistentData,
            Durability::Persistent,
            100,
            RiskLevel::Critical,
        )];

        let request = ExtensionPlanRequest::new(entries, 50_000);
        let plan = plan_extension_with_cost(&request, |_entry, target| (target as i64) * 2);

        assert_eq!(plan.selected.len(), 1);
        assert_eq!(plan.selected[0].estimated_cost_stroops, 100_000);
        assert_eq!(plan.total_cost_stroops, 100_000);
    }
}
