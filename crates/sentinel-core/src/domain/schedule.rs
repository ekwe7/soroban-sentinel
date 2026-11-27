//! TTL expiration calendar and maintenance-window scheduler for Soroban storage entries.

use crate::domain::{RiskLevel, ThresholdConfig};
use crate::usecases::scan::TTLReport;

/// Granularity for bucketing expiration projections into maintenance windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaintenanceWindowGranularity {
    /// One window per day of ledger horizon.
    Daily,
    /// One window per 7-day span.
    Weekly,
    /// Custom bucket size in ledgers per window.
    CustomLedgerBuckets { bucket_ledgers: u32 },
}

impl MaintenanceWindowGranularity {
    /// Number of ledgers per bucket for this granularity.
    pub fn bucket_ledgers(&self, _ledger_velocity_seconds: f64) -> u32 {
        match self {
            MaintenanceWindowGranularity::Daily => {
                // ~5s per ledger -> ~17280 ledgers/day; preserve the common
                // default of one day expressed as a fixed ledger count.
                17280u32
            }
            MaintenanceWindowGranularity::Weekly => 120960u32,
            MaintenanceWindowGranularity::CustomLedgerBuckets { bucket_ledgers } => *bucket_ledgers,
        }
    }
}

/// A single projected entry inside a maintenance window.
#[derive(Debug, Clone)]
pub struct ProjectedEntry {
    /// Contract ID owning the entry.
    pub contract_id: String,
    /// Key identifier (e.g. "persistent:COUNTER").
    pub key_identifier: String,
    /// Ledger at which this entry expires (live-until).
    pub live_until_ledger: u32,
    /// Projected risk classification at window time.
    pub projected_status: RiskLevel,
    /// Remaining ledgers from the latest ledger when projected.
    pub remaining_ledgers: i64,
}

/// A maintenance window covering a contiguous ledger range.
#[derive(Debug, Clone)]
pub struct MaintenanceWindow {
    /// Human-readable label, e.g. "daily-000", "weekly-000".
    pub label: String,
    /// First ledger in the window (inclusive).
    pub start_ledger: u32,
    /// Last ledger in the window (inclusive).
    pub end_ledger: u32,
    /// Critical entries projected to expire in this window.
    pub critical_entries: Vec<ProjectedEntry>,
    /// Warning entries projected to expire in this window.
    pub warning_entries: Vec<ProjectedEntry>,
    /// Whether the window contains any critical/archived projected entry.
    pub urgent: bool,
}

impl MaintenanceWindow {
    /// Returns true when the window contains at least one critical or archived entry.
    pub fn is_urgent(&self) -> bool {
        self.urgent || self.critical_entries.iter().any(|e| {
            e.projected_status == RiskLevel::Critical || e.projected_status == RiskLevel::Archived
        })
    }
}

/// Calendar of maintenance windows derived from a TTL report and threshold config.
pub struct ExpirationCalendar {
    /// Latest ledger used as the reference point for projections.
    pub latest_ledger: u32,
    /// Warning threshold in ledgers.
    pub warning_ledgers: u32,
    /// Critical threshold in ledgers.
    pub critical_ledgers: u32,
    /// Scheduled maintenance windows, ordered by start ledger.
    pub windows: Vec<MaintenanceWindow>,
}

impl ExpirationCalendar {
    /// Builds a new expiration calendar from a TTL report.
    pub fn new(
        report: &TTLReport,
        config: &ThresholdConfig,
        granularity: MaintenanceWindowGranularity,
    ) -> Self {
        let bucket_ledgers = granularity.bucket_ledgers(report.ledger_velocity_seconds);
        if bucket_ledgers == 0 || report.entries.is_empty() {
            return Self {
                latest_ledger: report.latest_ledger,
                warning_ledgers: config.warning_ledgers,
                critical_ledgers: config.critical_ledgers,
                windows: Vec::new(),
            };
        }

        let mut projected: Vec<ProjectedEntry> = report
            .entries
            .iter()
            .map(|entry| {
                let remaining_ledgers = (entry.live_until_ledger as i64)
                    .saturating_sub(report.latest_ledger as i64);
                let projected_status = if entry.live_until_ledger <= report.latest_ledger {
                    RiskLevel::Archived
                } else if remaining_ledgers <= config.critical_ledgers as i64 {
                    RiskLevel::Critical
                } else if remaining_ledgers <= config.warning_ledgers as i64 {
                    RiskLevel::Warn
                } else {
                    RiskLevel::Ok
                };
                ProjectedEntry {
                    contract_id: report.contract_id.clone(),
                    key_identifier: entry.key_identifier.clone(),
                    live_until_ledger: entry.live_until_ledger,
                    projected_status,
                    remaining_ledgers,
                }
            })
            .collect();

        // Sort projected entries by live_until_ledger so windows are stable.
        projected.sort_by(|a, b| a.live_until_ledger.cmp(&b.live_until_ledger));

        let latest = report.latest_ledger;
        let mut windows = Vec::new();
        let mut cursor = latest;
        let mut idx = 0usize;

        while idx < projected.len() {
            let window_start = cursor;
            let window_end = cursor.saturating_add(bucket_ledgers).saturating_sub(1);
            let window_end = window_end.max(window_start);

            let mut critical_entries = Vec::new();
            let mut warning_entries = Vec::new();

            while idx < projected.len() && projected[idx].live_until_ledger <= window_end {
                let e = &projected[idx];
                match e.projected_status {
                    RiskLevel::Critical | RiskLevel::Archived => {
                        critical_entries.push(e.clone());
                    }
                    RiskLevel::Warn => {
                        warning_entries.push(e.clone());
                    }
                    _ => {}
                }
                idx += 1;
            }

            let urgent = !critical_entries.is_empty();
            let label = match granularity {
                MaintenanceWindowGranularity::Daily => format!("daily-{:03}", windows.len()),
                MaintenanceWindowGranularity::Weekly => format!("weekly-{:03}", windows.len()),
                MaintenanceWindowGranularity::CustomLedgerBuckets { .. } => {
                    format!("ledger-bucket-{:03}", windows.len())
                }
            };

            windows.push(MaintenanceWindow {
                label,
                start_ledger: window_start,
                end_ledger: window_end,
                critical_entries,
                warning_entries,
                urgent,
            });

            cursor = window_end.saturating_add(1);
            if cursor <= window_end {
                break;
            }
        }

        Self {
            latest_ledger: report.latest_ledger,
            warning_ledgers: config.warning_ledgers,
            critical_ledgers: config.critical_ledgers,
            windows,
        }
    }

    /// Returns only the urgent (critical/archived) maintenance windows.
    pub fn urgent_windows(&self) -> Vec<MaintenanceWindow> {
        self.windows
            .iter()
            .cloned()
            .filter(|w| w.is_urgent())
            .collect()
    }
}

/// Convenience constructor used by the CLI/fuzz test harness when a report is
/// built from projected entries rather than live RPC data.
pub fn urgent_windows(calendar: &ExpirationCalendar) -> Vec<MaintenanceWindow> {
    calendar.urgent_windows()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Durability, EntryType, RiskLevel, ThresholdConfig};
    use crate::usecases::scan::{StorageEntryReport, TTLReport};

    fn sample_report(latest_ledger: u32, entries: Vec<StorageEntryReport>) -> TTLReport {
        TTLReport {
            contract_id: "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM".to_string(),
            network: "testnet".to_string(),
            latest_ledger,
            ledger_velocity_seconds: 5.0,
            burn_rate: crate::domain::burn_rate::BurnRate::default(),
            entries,
            overall_status: RiskLevel::Ok,
            archival_recommendations: Vec::new(),
            sac_asset_info: None,
            contract_code_hash: None,
            contract_bytecode_size: None,
            contract_upgrade: None,
        }
    }

    fn persistent_entry(
        key_identifier: &str,
        live_until_ledger: u32,
        remaining_ledgers: i64,
        status: RiskLevel,
    ) -> StorageEntryReport {
        StorageEntryReport {
            key_identifier: key_identifier.to_string(),
            entry_type: EntryType::PersistentData,
            durability: Durability::Persistent,
            byte_size: 12,
            live_until_ledger,
            remaining_ledgers,
            estimated_remaining_seconds: 0.0,
            remaining_days: 0.0,
            status,
            instance_storage_keys: Vec::new(),
        }
    }

    #[test]
    fn test_maintenance_window_urgent_flag() {
        let window = MaintenanceWindow {
            label: "weekly-000".to_string(),
            start_ledger: 100,
            end_ledger: 107,
            critical_entries: vec![
                ProjectedEntry {
                    contract_id: "CAAA".to_string(),
                    key_identifier: "persistent:COUNTER".to_string(),
                    live_until_ledger: 100,
                    projected_status: RiskLevel::Critical,
                    remaining_ledgers: 0,
                },
                ProjectedEntry {
                    contract_id: "CBBB".to_string(),
                    key_identifier: "persistent:BALANCE".to_string(),
                    live_until_ledger: 101,
                    projected_status: RiskLevel::Critical,
                    remaining_ledgers: 1,
                },
            ],
            warning_entries: vec![],
            urgent: true,
        };
        assert!(window.is_urgent());
    }

    #[test]
    fn test_expiration_calendar_daily_buckets() {
        let report = sample_report(5000, vec![
            persistent_entry("persistent:COUNTER", 5005, 5, RiskLevel::Warn),
            persistent_entry("persistent:BALANCE", 5001, 1, RiskLevel::Critical),
        ]);

        let config = ThresholdConfig::try_new(100, 30).unwrap();
        let calendar = ExpirationCalendar::new(&report, &config, MaintenanceWindowGranularity::Daily);

        assert_eq!(calendar.latest_ledger, 5000);
        assert_eq!(calendar.warning_ledgers, 100);
        assert_eq!(calendar.critical_ledgers, 30);
        assert!(!calendar.windows.is_empty());

        let critical_reached = calendar
            .windows
            .iter()
            .any(|w| w.critical_entries.iter().any(|e| {
                e.contract_id.contains("CAAA") && e.key_identifier.contains("BALANCE")
            }));
        assert!(critical_reached);
    }

    #[test]
    fn test_expiration_calendar_urgent_partition() {
        let report = sample_report(100, vec![
            persistent_entry("persistent:COUNTER", 100, 0, RiskLevel::Critical),
            persistent_entry("persistent:BALANCE", 100, 0, RiskLevel::Archived),
        ]);

        let config = ThresholdConfig::try_new(100, 30).unwrap();
        let calendar = ExpirationCalendar::new(&report, &config, MaintenanceWindowGranularity::Daily);

        let urgent = calendar.urgent_windows();
        assert!(!urgent.is_empty());
        let first = urgent.first().unwrap();
        assert_eq!(first.critical_entries.len(), 2);
        assert!(first.is_urgent());
    }

    #[test]
    fn test_expiration_calendar_weekly_buckets() {
        let report = sample_report(5000, vec![
            persistent_entry("persistent:COUNTER", 5035, 35, RiskLevel::Warn),
        ]);

        let config = ThresholdConfig::try_new(100, 30).unwrap();
        let calendar = ExpirationCalendar::new(&report, &config, MaintenanceWindowGranularity::Weekly);

        assert_eq!(calendar.windows.len(), 1);
        assert_eq!(calendar.windows.first().unwrap().label, "weekly-000");
    }

    #[test]
    fn test_expiration_calendar_custom_buckets() {
        let report = sample_report(1000, vec![
            persistent_entry("persistent:FOOTPRINT", 1010, 10, RiskLevel::Warn),
        ]);

        let config = ThresholdConfig::try_new(100, 30).unwrap();
        let calendar = ExpirationCalendar::new(
            &report,
            &config,
            MaintenanceWindowGranularity::CustomLedgerBuckets { bucket_ledgers: 5 },
        );

        assert_eq!(calendar.windows.first().unwrap().label, "ledger-bucket-000");
    }

    #[test]
    fn test_expiration_calendar_empty_report() {
        let report = sample_report(100, vec![]);

        let config = ThresholdConfig::try_new(100, 30).unwrap();
        let calendar = ExpirationCalendar::new(&report, &config, MaintenanceWindowGranularity::Daily);

        assert!(calendar.windows.is_empty());
    }
}
