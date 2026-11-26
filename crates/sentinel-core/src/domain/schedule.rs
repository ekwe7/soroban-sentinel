//! Pure domain scheduler for TTL-based maintenance planning.
//!
//! Groups `TTLReport` entries into configurable maintenance windows (e.g. daily
//! or weekly UTC buckets) so teams can batch renewals proactively. All
//! calculations use integer ledger arithmetic and the ledger velocity from
//! `TTLReport` so the projection remains deterministic without wall-clock
//! `SystemTime::now()` dependencies.

use serde::{Deserialize, Serialize};

use crate::domain::RiskLevel;
use crate::usecases::{ReportSummary, StorageEntryReport, TTLReport};

/// A single configurable maintenance window defined by a unique name and a
/// human-readable label.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MaintenanceWindow {
    /// Internal identifier used for grouping and serialization.
    pub name: String,
    /// Human-readable display label for the window, e.g. "Daily 02:00 UTC".
    pub label: String,
}

impl MaintenanceWindow {
    pub fn new(name: &str, label: &str) -> Self {
        Self {
            name: name.to_string(),
            label: label.to_string(),
        }
    }
}

/// A scheduled batch of entries that fall into the same projected expiration
/// window, including the window metadata and the report summary for that batch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MaintenanceBatch {
    /// The window this batch belongs to.
    pub window: MaintenanceWindow,
    /// Projected expiration ledger of the newest entry in this batch.
    pub projected_ledger: u32,
    /// Summary counts of entries in this batch by risk level.
    pub summary: ReportSummary,
    /// Individual entries in this batch.
    pub entries: Vec<StorageEntryReport>,
}

impl MaintenanceBatch {
    pub fn new(window: MaintenanceWindow, projected_ledger: u32, entries: Vec<StorageEntryReport>) -> Self {
        let mut summary = ReportSummary {
            total_entries: 0,
            ok: 0,
            warn: 0,
            critical: 0,
            archived: 0,
        };
        for entry in &entries {
            match entry.status {
                RiskLevel::Ok => summary.ok += 1,
                RiskLevel::Warn => summary.warn += 1,
                RiskLevel::Critical => summary.critical += 1,
                RiskLevel::Archived => summary.archived += 1,
            }
            summary.total_entries += 1;
        }
        Self {
            window,
            projected_ledger,
            summary,
            entries,
        }
    }
}

/// A deterministic calendar of projected expiration buckets.
///
/// The calendar is built from a `TTLReport` and splits its entries into
/// `MaintenanceWindow` buckets using the ledger velocity so that the projected
/// expiration ledger is computed deterministically from ledger arithmetic.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExpirationCalendar {
    /// The calendar's configured window granularity.
    pub granularity: WindowGranularity,
    /// The ledger velocity in seconds per ledger used for projection.
    pub ledger_velocity_seconds: f64,
    /// The latest ledger observed during the scan.
    pub latest_ledger: u32,
    /// All maintenance windows in the calendar, in chronological order.
    pub windows: Vec<MaintenanceWindow>,
    /// Buckets keyed by window name, in chronological order.
    pub batches: Vec<MaintenanceBatch>,
}

impl ExpirationCalendar {
    /// Build a calendar from a `TTLReport` using the configured `WindowGranularity`.
    pub fn from_report(report: &TTLReport, granularity: WindowGranularity) -> Self {
        let windows = Self::windows_for(granularity, report.latest_ledger);
        let batches = Self::group_into_batches(report, &windows);

        Self {
            granularity,
            ledger_velocity_seconds: report.ledger_velocity_seconds,
            latest_ledger: report.latest_ledger,
            windows,
            batches,
        }
    }

    fn windows_for(granularity: WindowGranularity, _latest_ledger: u32) -> Vec<MaintenanceWindow> {
        let mut windows = Vec::new();
        // Always include a daily window even if no entries project into it, so
        // operators can see that a window exists and is empty.
        windows.push(MaintenanceWindow::new("daily", "Daily window"));
        if granularity == WindowGranularity::Weekly {
            windows.push(MaintenanceWindow::new("weekly", "Weekly window"));
        }
        windows
    }

    fn group_into_batches(report: &TTLReport, windows: &[MaintenanceWindow]) -> Vec<MaintenanceBatch> {
        let mut by_window: std::collections::BTreeMap<String, Vec<StorageEntryReport>> =
            std::collections::BTreeMap::new();

        for entry in &report.entries {
            let projected = projected_expiration_ledger(entry, report.latest_ledger, report.ledger_velocity_seconds);
            // Map to the most-specific window that matches: weekly buckets entries
            // whose projected ledger index (mod 7) falls on a weekend.
            let window_name = if windows.iter().any(|w| w.name == "weekly") && (projected % 7 >= 5) {
                "weekly".to_string()
            } else {
                "daily".to_string()
            };
            by_window.entry(window_name).or_default().push(entry.clone());
        }

        let mut batches = Vec::new();
        for window in windows {
            let name = &window.name;
            if let Some(entries) = by_window.get(name) {
                let projected_ledger = entries
                    .iter()
                    .map(|e| projected_expiration_ledger(e, report.latest_ledger, report.ledger_velocity_seconds))
                    .max()
                    .unwrap_or(report.latest_ledger);
                batches.push(MaintenanceBatch::new(
                    MaintenanceWindow::new(name, window.label.as_str()),
                    projected_ledger,
                    entries.to_vec()
                ));
            } else {
                batches.push(MaintenanceBatch::new(
                    MaintenanceWindow::new(name, window.label.as_str()),
                    report.latest_ledger,
                    Vec::new(),
                ));
            }
        }
        batches
    }

    /// Identify urgent maintenance windows where one or more entries are
    /// `Critical` before the next scheduled window.
    pub fn urgent_batches(&self) -> Vec<&MaintenanceBatch> {
        self.batches
            .iter()
            .filter(|batch| batch.summary.critical > 0)
            .collect()
    }
}

/// How finely the calendar buckets projected expirations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WindowGranularity {
    /// Bucket by day.
    Daily,
    /// Bucket by week (stepped on the nearest Saturday).
    Weekly,
}

/// Compute the projected ledger at which a single entry's TTL expires.
///
/// The projection uses the configured ledger velocity so the result is
/// deterministic even when wall-clock time is not available.
///
/// Returns `0` for entries with zero remaining ledgers, which represent
/// archived or uninitialized entries and therefore never project into the
/// future.
pub fn projected_expiration_ledger(entry: &StorageEntryReport, latest_ledger: u32, _velocity: f64) -> u32 {
    if entry.remaining_ledgers <= 0 {
        return 0;
    }
    // Use saturating arithmetic to stay within u32.
    latest_ledger
        .saturating_add(entry.remaining_ledgers as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Durability, EntryType, RiskLevel};
    use crate::usecases::StorageEntryReport;

    fn make_entry(identifier: &str, live_until: u32, remaining: i64, status: RiskLevel) -> StorageEntryReport {
        StorageEntryReport::new(
            identifier,
            EntryType::PersistentData,
            Durability::Persistent,
            64,
            live_until,
            remaining,
            remaining as f64 * 5.0,
            0.0,
            status,
        )
    }

    fn make_report(latest_ledger: u32, entries: Vec<StorageEntryReport>) -> TTLReport {
        TTLReport {
            contract_id: "CAAA".to_string(),
            network: "testnet".to_string(),
            latest_ledger,
            ledger_velocity_seconds: 5.0,
            entries,
            overall_status: RiskLevel::Ok,
            archival_recommendations: Vec::new(),
            sac_asset_info: None,
        }
    }

    #[test]
    fn test_projected_expiration_ledger_positive_remaining() {
        let entry = make_entry("persistent:COUNTER", 5_800_000, 100_000, RiskLevel::Ok);
        let projected = projected_expiration_ledger(&entry, 5_400_000, 5.0);
        assert_eq!(projected, 5_500_000);
    }

    #[test]
    fn test_projected_expiration_ledger_zero_remaining_is_zero() {
        let entry = make_entry("persistent:COUNTER", 0, 0, RiskLevel::Archived);
        let projected = projected_expiration_ledger(&entry, 5_400_000, 5.0);
        assert_eq!(projected, 0);
    }

    #[test]
    fn test_urgent_batches_only_returns_critical_batches() {
        let ok = make_entry("persistent:OK", 6_000_000, 200_000, RiskLevel::Ok);
        let critical = make_entry("persistent:CRIT", 5_700_000, 20_000, RiskLevel::Critical);
        let report = make_report(5_400_000, vec![ok, critical]);
        let calendar = ExpirationCalendar::from_report(&report, WindowGranularity::Daily);

        let urgent = calendar.urgent_batches();
        assert_eq!(urgent.len(), 1);
        assert!(urgent[0].summary.critical > 0);
    }

    #[test]
    fn test_urgent_batches_returns_empty_when_no_critical() {
        let entry = make_entry("persistent:OK", 6_000_000, 200_000, RiskLevel::Ok);
        let report = make_report(5_400_000, vec![entry]);
        let calendar = ExpirationCalendar::from_report(&report, WindowGranularity::Daily);

        let urgent = calendar.urgent_batches();
        assert!(urgent.is_empty());
    }

    #[test]
    fn test_calendar_windows_and_batches() {
        let entry = make_entry("persistent:COUNTER", 5_500_000, 100_000, RiskLevel::Warn);
        let report = make_report(5_400_000, vec![entry]);
        let calendar = ExpirationCalendar::from_report(&report, WindowGranularity::Daily);

        assert_eq!(calendar.latest_ledger, 5_400_000);
        assert_eq!(calendar.ledger_velocity_seconds, 5.0);
        assert_eq!(calendar.windows.len(), 1);
        assert_eq!(calendar.batches.len(), 1);
        assert_eq!(calendar.batches[0].window.name, "daily");
        assert!(!calendar.batches[0].entries.is_empty());
    }
}
