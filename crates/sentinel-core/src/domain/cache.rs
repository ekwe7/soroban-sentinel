//! Thread-safe, memory-bounded cache for transaction simulation results.
//!
//! Cache keys are SHA-256 hashes of the encoded transaction footprint, so the same
//! footprint always maps to the same key. Entries are evicted when their recorded
//! ledger age is strictly greater than `max_ledger_age` (an entry exactly at the
//! horizon remains valid). When the cache is full, the least-recently-used entry
//! is evicted. All locks are held only around in-memory map access; callers must
//! perform RPC simulation outside the cache.

use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::RwLock;

/// A cached simulation result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SimulationCacheEntry {
    pub min_resource_fee: u64,
    pub latest_ledger: u32,
    pub cpu_instructions: u64,
    pub memory_bytes: u64,
}

/// SHA-256 key derived from an encoded transaction footprint.
pub type SimulationCacheKey = [u8; 32];

/// Deterministically derives a cache key from footprint XDR.
pub fn footprint_cache_key(footprint_xdr: &str) -> SimulationCacheKey {
    let mut digest = Sha256::new();
    digest.update(footprint_xdr.as_bytes());
    let result = digest.finalize();
    let mut key = [0u8; 32];
    key.copy_from_slice(&result);
    key
}

#[derive(Debug, Clone)]
struct CacheValue {
    entry: SimulationCacheEntry,
    last_used: u64,
}

/// Thread-safe in-memory simulation cache.
/// Eviction policy: stale entries are removed first; otherwise the least-recently-used entry is evicted.
#[derive(Debug)]
pub struct SimulationCache {
    max_entries: usize,
    max_ledger_age: u32,
    values: RwLock<HashMap<SimulationCacheKey, CacheValue>>,
    clock: AtomicU64,
}

impl SimulationCache {
    pub fn new(max_entries: usize, max_ledger_age: u32) -> Self {
        Self {
            max_entries,
            max_ledger_age,
            values: RwLock::new(HashMap::new()),
            clock: AtomicU64::new(0),
        }
    }

    pub fn len(&self) -> usize {
        self.values.read().map_or_else(
            |poisoned| poisoned.into_inner().len(),
            |values| values.len(),
        )
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn max_entries(&self) -> usize {
        self.max_entries
    }

    pub fn max_ledger_age(&self) -> u32 {
        self.max_ledger_age
    }

    /// Returns a live entry. Equal to the ledger-age boundary remains valid.
    pub fn get(
        &self,
        key: &SimulationCacheKey,
        latest_ledger: u32,
    ) -> Option<SimulationCacheEntry> {
        let mut values = self
            .values
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let stale = match values.get(key) {
            Some(value) => {
                latest_ledger.saturating_sub(value.entry.latest_ledger) > self.max_ledger_age
            }
            None => false,
        };
        if stale {
            values.remove(key);
            return None;
        }
        if let Some(value) = values.get_mut(key) {
            let tick = self.clock.fetch_add(1, Ordering::Relaxed).saturating_add(1);
            value.last_used = tick;
            return Some(value.entry.clone());
        }
        None
    }

    pub fn insert(&self, key: SimulationCacheKey, entry: SimulationCacheEntry) {
        let mut values = self
            .values
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let tick = self.clock.fetch_add(1, Ordering::Relaxed).saturating_add(1);
        values.insert(
            key,
            CacheValue {
                entry,
                last_used: tick,
            },
        );

        if values.len() > self.max_entries {
            if let Some(oldest) = values
                .iter()
                .min_by_key(|(_, value)| value.last_used)
                .map(|(key, _)| *key)
            {
                values.remove(&oldest);
            }
        }
    }

    pub fn evict_stale(&self, latest_ledger: u32) {
        let mut values = self
            .values
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        values.retain(|_, value| {
            latest_ledger.saturating_sub(value.entry.latest_ledger) <= self.max_ledger_age
        });
    }

    pub fn clear(&self) {
        self.values
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(ledger: u32) -> SimulationCacheEntry {
        SimulationCacheEntry {
            min_resource_fee: 123,
            latest_ledger: ledger,
            cpu_instructions: 456,
            memory_bytes: 789,
        }
    }

    #[test]
    fn deterministic_keys_and_different_footprints() {
        let a = footprint_cache_key("A");
        assert_eq!(a, footprint_cache_key("A"));
        assert_ne!(a, footprint_cache_key("B"));
    }

    #[test]
    fn exact_ledger_age_boundary_and_eviction() {
        let cache = SimulationCache::new(10, 100);
        let key = footprint_cache_key("footprint");
        cache.insert(key, entry(1_000));
        assert_eq!(cache.get(&key, 1_100), Some(entry(1_000)));
        assert_eq!(cache.get(&key, 1_101), None);
    }

    #[test]
    fn evicts_least_recently_used_entry_when_full() {
        let cache = SimulationCache::new(2, 100);
        let first = footprint_cache_key("first");
        let second = footprint_cache_key("second");
        let third = footprint_cache_key("third");
        cache.insert(first, entry(1_000));
        cache.insert(second, entry(1_000));
        assert_eq!(cache.get(&first, 1_000), Some(entry(1_000)));
        cache.insert(third, entry(1_000));
        assert_eq!(cache.get(&first, 1_000), Some(entry(1_000)));
        assert_eq!(cache.get(&second, 1_000), None);
        assert_eq!(cache.get(&third, 1_000), Some(entry(1_000)));
    }

    #[test]
    fn zero_capacity_never_retains_entries() {
        let cache = SimulationCache::new(0, 100);
        let key = footprint_cache_key("footprint");
        cache.insert(key, entry(1_000));
        assert_eq!(cache.len(), 0);
        assert_eq!(cache.get(&key, 1_000), None);
    }

    #[test]
    fn evict_stale_removes_only_entries_past_the_boundary() {
        let cache = SimulationCache::new(10, 100);
        let fresh = footprint_cache_key("fresh");
        let stale = footprint_cache_key("stale");
        cache.insert(fresh, entry(1_000));
        cache.insert(stale, entry(800));
        cache.evict_stale(1_100);
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.get(&fresh, 1_100), Some(entry(1_000)));
        assert_eq!(cache.get(&stale, 1_100), None);
    }

    #[test]
    fn bounds_entries_under_concurrent_inserts() {
        let cache = std::sync::Arc::new(SimulationCache::new(32, 100));
        let mut tasks = Vec::new();
        for i in 0..128u32 {
            let cache = cache.clone();
            tasks.push(std::thread::spawn(move || {
                let key = footprint_cache_key(&format!("footprint-{i}"));
                cache.insert(key, entry(1_000));
            }));
        }
        for task in tasks {
            task.join().expect("cache task");
        }
        assert_eq!(cache.len(), 32);
    }
}
