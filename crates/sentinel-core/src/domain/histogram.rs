//! Expiration horizons for caller-supplied entries; no network access.

use serde::{Deserialize, Serialize};

use super::durability_migration::{RentAnalysisError, RentRates};
use super::{calculate_remaining_ledgers, StorageEntry, SECONDS_PER_DAY};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExpirationBucket {
    Immediate,
    ShortTerm,
    MediumTerm,
    LongTerm,
    Archived,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistogramBucket {
    pub bucket: ExpirationBucket,
    pub count: usize,
    pub total_bytes: u128,
    /// Rent for the positive remaining-ledger horizon at supplied rates.
    /// Byte sizes are assumed to be fee-billed sizes for the modeled protocol.
    pub projected_rent_stroops: u128,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExpirationHistogram {
    pub buckets: Vec<HistogramBucket>,
    pub total_count: usize,
    pub p10_remaining_ledgers: Option<i64>,
    pub p50_remaining_ledgers: Option<i64>,
    pub p90_remaining_ledgers: Option<i64>,
}

impl ExpirationHistogram {
    /// Bounds are [0,1), [1,7), [7,30), [30,90), [90,infinity) days.
    /// Archived is the issue's 90d+ bucket name; it does not imply eviction.
    /// Already-expired entries belong to Immediate. Percentiles use nearest rank.
    pub fn aggregate(
        entries: &[StorageEntry],
        latest_ledger: u32,
        ledger_seconds: f64,
        rates: &RentRates,
    ) -> Result<Self, RentAnalysisError> {
        if !ledger_seconds.is_finite() || ledger_seconds <= 0.0 {
            return Err(RentAnalysisError::InvalidHorizon);
        }
        let mut buckets: Vec<_> = [
            ExpirationBucket::Immediate,
            ExpirationBucket::ShortTerm,
            ExpirationBucket::MediumTerm,
            ExpirationBucket::LongTerm,
            ExpirationBucket::Archived,
        ]
        .into_iter()
        .map(|bucket| HistogramBucket {
            bucket,
            count: 0,
            total_bytes: 0,
            projected_rent_stroops: 0,
        })
        .collect();
        let mut remaining = Vec::with_capacity(entries.len());
        for entry in entries {
            let ledgers = calculate_remaining_ledgers(entry.live_until_ledger(), latest_ledger);
            remaining.push(ledgers);
            let days = ledgers.max(0) as f64 * ledger_seconds / SECONDS_PER_DAY;
            let index = [1.0, 7.0, 30.0, 90.0]
                .iter()
                .position(|bound| days < *bound)
                .unwrap_or(4);
            let bucket = &mut buckets[index];
            bucket.count += 1;
            bucket.total_bytes += entry.byte_size() as u128;
            let bytes =
                u32::try_from(entry.byte_size()).map_err(|_| RentAnalysisError::Overflow)?;
            let rent = rates.entry_rent(bytes, ledgers.max(0) as u32, entry.durability())?;
            bucket.projected_rent_stroops = bucket
                .projected_rent_stroops
                .checked_add(rent)
                .ok_or(RentAnalysisError::Overflow)?;
        }
        remaining.sort_unstable();
        let percentile = |percent: usize| {
            if remaining.is_empty() {
                None
            } else {
                let rank = (remaining.len() as u128 * percent as u128).div_ceil(100) as usize;
                Some(remaining[rank - 1])
            }
        };
        Ok(Self {
            buckets,
            total_count: entries.len(),
            p10_remaining_ledgers: percentile(10),
            p50_remaining_ledgers: percentile(50),
            p90_remaining_ledgers: percentile(90),
        })
    }
}
