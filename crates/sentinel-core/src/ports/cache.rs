//! Thread-safe in-memory caching decorator for [`LedgerReader`] implementations.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tokio::time::Instant;

use crate::error::SentinelResult;
use crate::ports::reader::{LatestLedgerInfo, LedgerReader, RawLedgerEntry};

/// Cumulative operational statistics for a [`CachedLedgerReader`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CacheStats {
    /// Total count of successful cache hits across all operations.
    pub hits: u64,
    /// Total count of cache misses that required querying the inner reader.
    pub misses: u64,
    /// Total count of expired entries evicted upon stale access.
    pub evictions: u64,
}

#[derive(Clone)]
struct CacheItem<T> {
    value: T,
    expires_at: Instant,
}

/// A thread-safe caching decorator wrapping any [`LedgerReader`].
///
/// Reduces redundant network/RPC load by caching `get_latest_ledger` and
/// individual `get_ledger_entries` responses for a configurable TTL `max_age`.
pub struct CachedLedgerReader<R: LedgerReader> {
    inner: R,
    max_age: Duration,
    latest_ledger_cache: Arc<RwLock<Option<CacheItem<LatestLedgerInfo>>>>,
    entry_cache: Arc<RwLock<HashMap<String, CacheItem<RawLedgerEntry>>>>,
    hits: Arc<AtomicU64>,
    misses: Arc<AtomicU64>,
    evictions: Arc<AtomicU64>,
}

impl<R: LedgerReader> CachedLedgerReader<R> {
    /// Creates a new `CachedLedgerReader` wrapping the given inner reader with a specified `max_age`.
    pub fn new(inner: R, max_age: Duration) -> Self {
        Self {
            inner,
            max_age,
            latest_ledger_cache: Arc::new(RwLock::new(None)),
            entry_cache: Arc::new(RwLock::new(HashMap::new())),
            hits: Arc::new(AtomicU64::new(0)),
            misses: Arc::new(AtomicU64::new(0)),
            evictions: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Returns a snapshot of current cache statistics (`hits`, `misses`, `evictions`).
    pub fn cache_stats(&self) -> CacheStats {
        CacheStats {
            hits: self.hits.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
            evictions: self.evictions.load(Ordering::Relaxed),
        }
    }

    /// Provides reference access to the underlying [`LedgerReader`].
    pub fn inner(&self) -> &R {
        &self.inner
    }

    /// Configured maximum cache age.
    pub fn max_age(&self) -> Duration {
        self.max_age
    }

    /// Clears all cached items and entries.
    pub async fn clear(&self) {
        let mut latest = self.latest_ledger_cache.write().await;
        *latest = None;
        let mut entries = self.entry_cache.write().await;
        entries.clear();
    }
}

#[async_trait]
impl<R: LedgerReader> LedgerReader for CachedLedgerReader<R> {
    async fn get_latest_ledger(&self) -> SentinelResult<LatestLedgerInfo> {
        let now = Instant::now();
        {
            let guard = self.latest_ledger_cache.read().await;
            if let Some(ref item) = *guard {
                if now < item.expires_at {
                    self.hits.fetch_add(1, Ordering::Relaxed);
                    return Ok(item.value);
                }
            }
        }

        // Cache miss or expired entry
        let fresh = self.inner.get_latest_ledger().await?;
        let mut guard = self.latest_ledger_cache.write().await;
        if let Some(ref item) = *guard {
            if now >= item.expires_at {
                self.evictions.fetch_add(1, Ordering::Relaxed);
            }
        }
        self.misses.fetch_add(1, Ordering::Relaxed);
        *guard = Some(CacheItem {
            value: fresh,
            expires_at: now + self.max_age,
        });
        Ok(fresh)
    }

    async fn get_ledger_entries(
        &self,
        keys_xdr_base64: &[String],
    ) -> SentinelResult<Vec<RawLedgerEntry>> {
        if keys_xdr_base64.is_empty() {
            return Ok(Vec::new());
        }

        let now = Instant::now();
        let mut cached_results = Vec::new();
        let mut missed_keys = Vec::new();
        let mut expired_keys = Vec::new();

        {
            let guard = self.entry_cache.read().await;
            for key in keys_xdr_base64 {
                if let Some(item) = guard.get(key) {
                    if now < item.expires_at {
                        cached_results.push(item.value.clone());
                        self.hits.fetch_add(1, Ordering::Relaxed);
                    } else {
                        expired_keys.push(key.clone());
                        missed_keys.push(key.clone());
                        self.misses.fetch_add(1, Ordering::Relaxed);
                    }
                } else {
                    missed_keys.push(key.clone());
                    self.misses.fetch_add(1, Ordering::Relaxed);
                }
            }
        }

        if !expired_keys.is_empty() {
            self.evictions
                .fetch_add(expired_keys.len() as u64, Ordering::Relaxed);
        }

        if missed_keys.is_empty() {
            return Ok(cached_results);
        }

        // Fetch only missed/expired keys from inner reader
        let fetched_entries = self.inner.get_ledger_entries(&missed_keys).await?;

        {
            let mut guard = self.entry_cache.write().await;
            // Remove evicted keys
            for expired_key in &expired_keys {
                guard.remove(expired_key);
            }
            // Populate newly fetched entries
            for entry in &fetched_entries {
                guard.insert(
                    entry.key_xdr_base64.clone(),
                    CacheItem {
                        value: entry.clone(),
                        expires_at: now + self.max_age,
                    },
                );
            }
        }

        cached_results.extend(fetched_entries);
        Ok(cached_results)
    }

    async fn get_ledger_close_velocity(&self, latest_ledger: u32) -> SentinelResult<Option<f64>> {
        self.inner.get_ledger_close_velocity(latest_ledger).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::mock::MockLedgerReader;

    #[tokio::test]
    async fn test_latest_ledger_caching_and_expiration() {
        tokio::time::pause();

        let mock = MockLedgerReader::new().with_latest_ledger(5_000_000, 22);
        let cached_reader = CachedLedgerReader::new(mock, Duration::from_secs(60));

        // 1st call: Cache miss
        let first = cached_reader.get_latest_ledger().await.unwrap();
        assert_eq!(first.sequence, 5_000_000);
        assert_eq!(cached_reader.cache_stats().misses, 1);
        assert_eq!(cached_reader.cache_stats().hits, 0);

        // 2nd call before max_age: Cache hit
        tokio::time::advance(Duration::from_secs(30)).await;
        let second = cached_reader.get_latest_ledger().await.unwrap();
        assert_eq!(second.sequence, 5_000_000);
        assert_eq!(cached_reader.cache_stats().hits, 1);
        assert_eq!(cached_reader.cache_stats().misses, 1);

        // 3rd call after max_age: Expired -> eviction + miss
        tokio::time::advance(Duration::from_secs(35)).await;
        let third = cached_reader.get_latest_ledger().await.unwrap();
        assert_eq!(third.sequence, 5_000_000);
        assert_eq!(cached_reader.cache_stats().hits, 1);
        assert_eq!(cached_reader.cache_stats().misses, 2);
        assert_eq!(cached_reader.cache_stats().evictions, 1);
    }

    #[tokio::test]
    async fn test_ledger_entries_batching_and_miss_merging() {
        tokio::time::pause();

        let entry1 = RawLedgerEntry::new("key_1", "val_1", 5_100_000, 5_000_000);
        let entry2 = RawLedgerEntry::new("key_2", "val_2", 5_200_000, 5_000_000);
        let entry3 = RawLedgerEntry::new("key_3", "val_3", 5_300_000, 5_000_000);

        let mock = MockLedgerReader::new()
            .with_entry(entry1.clone())
            .with_entry(entry2.clone())
            .with_entry(entry3.clone());

        let cached_reader = CachedLedgerReader::new(mock, Duration::from_secs(60));

        // Request keys 1 & 2 -> 2 misses
        let res1 = cached_reader
            .get_ledger_entries(&["key_1".to_string(), "key_2".to_string()])
            .await
            .unwrap();
        assert_eq!(res1.len(), 2);
        assert_eq!(cached_reader.cache_stats().misses, 2);
        assert_eq!(cached_reader.cache_stats().hits, 0);

        // Request keys 1, 2, and 3 -> 2 hits (1 & 2), 1 miss (3)
        let res2 = cached_reader
            .get_ledger_entries(&[
                "key_1".to_string(),
                "key_2".to_string(),
                "key_3".to_string(),
            ])
            .await
            .unwrap();
        assert_eq!(res2.len(), 3);
        assert_eq!(cached_reader.cache_stats().hits, 2);
        assert_eq!(cached_reader.cache_stats().misses, 3);
        assert_eq!(cached_reader.cache_stats().evictions, 0);

        // Advance past expiration -> keys should expire and evict on subsequent read
        tokio::time::advance(Duration::from_secs(65)).await;
        let res3 = cached_reader
            .get_ledger_entries(&["key_1".to_string()])
            .await
            .unwrap();
        assert_eq!(res3.len(), 1);
        assert_eq!(cached_reader.cache_stats().hits, 2);
        assert_eq!(cached_reader.cache_stats().misses, 4);
        assert_eq!(cached_reader.cache_stats().evictions, 1);
    }

    #[tokio::test]
    async fn test_empty_keys_query_returns_empty() {
        let mock = MockLedgerReader::new();
        let cached_reader = CachedLedgerReader::new(mock, Duration::from_secs(60));
        let res = cached_reader.get_ledger_entries(&[]).await.unwrap();
        assert!(res.is_empty());
        assert_eq!(cached_reader.cache_stats().hits, 0);
        assert_eq!(cached_reader.cache_stats().misses, 0);
    }
}
