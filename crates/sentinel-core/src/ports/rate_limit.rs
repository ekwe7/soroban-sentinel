//! Client-side token-bucket rate-limiting decorator for [`LedgerReader`] implementations.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::Mutex;
use tokio::time::Instant;

use crate::error::SentinelResult;
use crate::ports::reader::{LatestLedgerInfo, LedgerReader, RawLedgerEntry};

/// Default sustained RPC request rate (requests per second).
pub const DEFAULT_RATE_LIMIT_RPS: f64 = 10.0;

/// Default maximum token-bucket burst capacity.
pub const DEFAULT_RATE_LIMIT_BURST_SIZE: u32 = 10;

/// Upper bound on the per-token refill interval (1 year) to prevent `Instant`/`Duration` overflow.
const MAX_REFILL_INTERVAL_SECS: f64 = 365.0 * 24.0 * 60.0 * 60.0;

/// Validation errors returned when constructing or validating a [`RateLimiterConfig`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RateLimitConfigError {
    /// `requests_per_second` was zero, negative, non-finite, or too small to represent a valid interval.
    #[error("Invalid requests_per_second: {0}")]
    InvalidRequestsPerSecond(String),
    /// `burst_size` was zero.
    #[error("Invalid burst_size: must be greater than 0")]
    InvalidBurstSize,
}

/// Configuration for token-bucket client-side RPC rate limiting.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RateLimiterConfig {
    /// Sustained token refill rate in requests per second (must be finite and `> 0`).
    pub requests_per_second: f64,
    /// Maximum burst capacity of the token bucket (must be `> 0`).
    pub burst_size: u32,
}

impl Default for RateLimiterConfig {
    fn default() -> Self {
        Self {
            requests_per_second: DEFAULT_RATE_LIMIT_RPS,
            burst_size: DEFAULT_RATE_LIMIT_BURST_SIZE,
        }
    }
}

impl RateLimiterConfig {
    /// Constructs and validates a new [`RateLimiterConfig`].
    pub fn new(requests_per_second: f64, burst_size: u32) -> Result<Self, RateLimitConfigError> {
        let config = Self {
            requests_per_second,
            burst_size,
        };
        config.validate()?;
        Ok(config)
    }

    /// Validates that both `requests_per_second` and `burst_size` are strictly positive and valid.
    pub fn validate(&self) -> Result<(), RateLimitConfigError> {
        if !self.requests_per_second.is_finite() || self.requests_per_second <= 0.0 {
            return Err(RateLimitConfigError::InvalidRequestsPerSecond(format!(
                "must be finite and greater than 0, got {}",
                self.requests_per_second
            )));
        }

        let interval_secs = 1.0 / self.requests_per_second;
        if !interval_secs.is_finite()
            || interval_secs > MAX_REFILL_INTERVAL_SECS
            || Duration::try_from_secs_f64(interval_secs).is_err()
        {
            return Err(RateLimitConfigError::InvalidRequestsPerSecond(format!(
                "rate {} is too small to represent a valid pacing interval",
                self.requests_per_second
            )));
        }

        if self.burst_size == 0 {
            return Err(RateLimitConfigError::InvalidBurstSize);
        }

        Ok(())
    }
}

#[derive(Debug)]
struct TokenBucketState {
    tokens: f64,
    last_refill: Option<Instant>,
}

impl TokenBucketState {
    fn new(burst_size: u32) -> Self {
        Self {
            tokens: f64::from(burst_size),
            last_refill: None,
        }
    }

    fn refill(&mut self, now: Instant, rate: f64, capacity: f64) {
        let last = *self.last_refill.get_or_insert(now);
        let elapsed = now.saturating_duration_since(last);
        if !elapsed.is_zero() {
            let added = elapsed.as_secs_f64() * rate;
            self.tokens = (self.tokens + added).min(capacity);
            self.last_refill = Some(now);
        }
    }
}

#[derive(Debug)]
struct TokenBucket {
    config: RateLimiterConfig,
    state: Mutex<TokenBucketState>,
}

impl TokenBucket {
    fn new(config: RateLimiterConfig) -> Self {
        Self {
            config,
            state: Mutex::new(TokenBucketState::new(config.burst_size)),
        }
    }

    /// Acquires a single token, sleeping only as long as needed to refill the deficit.
    ///
    /// Holding the FIFO-fair `tokio::sync::Mutex` across the sleep prevents starvation and
    /// thundering herds across concurrent callers while keeping token state cancellation-safe:
    /// if a sleeping caller is dropped, no tokens are lost and the next waiter immediately
    /// refills from `Instant::now()`.
    async fn acquire(&self) {
        let capacity = f64::from(self.config.burst_size);
        let rate = self.config.requests_per_second;

        let mut state = self.state.lock().await;
        let now = Instant::now();
        state.refill(now, rate, capacity);

        if state.tokens >= 1.0 {
            state.tokens -= 1.0;
            return;
        }

        let deficit = 1.0 - state.tokens;
        let wait_secs = deficit / rate;
        let mut wait = Duration::try_from_secs_f64(wait_secs).unwrap_or(Duration::ZERO);
        if wait_secs > 0.0 && wait.as_secs_f64() * rate < deficit {
            wait = wait.saturating_add(Duration::from_nanos(1));
        }

        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
            state.refill(Instant::now(), rate, capacity);
        }
        state.tokens = (state.tokens - 1.0).max(0.0);
    }

    async fn available_tokens(&self) -> f64 {
        let capacity = f64::from(self.config.burst_size);
        let rate = self.config.requests_per_second;
        let mut state = self.state.lock().await;
        state.refill(Instant::now(), rate, capacity);
        state.tokens
    }
}

/// A concurrency-safe token-bucket rate-limiting wrapper around any [`LedgerReader`].
///
/// Gates both [`LedgerReader::get_latest_ledger`] and [`LedgerReader::get_ledger_entries`]
/// (consuming one token per RPC batch call) so multi-contract scans and watch loops stay
/// within Soroban RPC provider RPS and burst quotas.
#[derive(Debug, Clone)]
pub struct RateLimitedLedgerReader<R> {
    inner: R,
    bucket: Arc<TokenBucket>,
}

impl<R: LedgerReader> RateLimitedLedgerReader<R> {
    /// Creates a new `RateLimitedLedgerReader` wrapping `inner` with the given validated `config`.
    pub fn new(inner: R, config: RateLimiterConfig) -> Result<Self, RateLimitConfigError> {
        config.validate()?;
        Ok(Self {
            inner,
            bucket: Arc::new(TokenBucket::new(config)),
        })
    }

    /// Returns the active [`RateLimiterConfig`].
    pub fn config(&self) -> RateLimiterConfig {
        self.bucket.config
    }

    /// Provides shared reference access to the underlying [`LedgerReader`].
    pub fn inner(&self) -> &R {
        &self.inner
    }

    /// Consumes the wrapper and returns the underlying [`LedgerReader`].
    pub fn into_inner(self) -> R {
        self.inner
    }

    /// Returns the number of tokens currently available after refilling up to `Instant::now()`.
    pub async fn available_tokens(&self) -> f64 {
        self.bucket.available_tokens().await
    }
}

#[async_trait]
impl<R: LedgerReader> LedgerReader for RateLimitedLedgerReader<R> {
    async fn get_latest_ledger(&self) -> SentinelResult<LatestLedgerInfo> {
        self.bucket.acquire().await;
        self.inner.get_latest_ledger().await
    }

    async fn get_ledger_entries(
        &self,
        keys_xdr_base64: &[String],
    ) -> SentinelResult<Vec<RawLedgerEntry>> {
        if keys_xdr_base64.is_empty() {
            return Ok(Vec::new());
        }
        self.bucket.acquire().await;
        self.inner.get_ledger_entries(keys_xdr_base64).await
    }

    async fn get_ledger_close_velocity(&self, latest_ledger: u32) -> SentinelResult<Option<f64>> {
        self.inner.get_ledger_close_velocity(latest_ledger).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::SentinelError;
    use crate::ports::mock::MockLedgerReader;

    #[test]
    fn test_config_validation_rejects_zero_and_invalid_values() {
        assert!(matches!(
            RateLimiterConfig::new(0.0, 5),
            Err(RateLimitConfigError::InvalidRequestsPerSecond(_))
        ));
        assert!(matches!(
            RateLimiterConfig::new(-0.0, 5),
            Err(RateLimitConfigError::InvalidRequestsPerSecond(_))
        ));
        assert!(matches!(
            RateLimiterConfig::new(-2.5, 5),
            Err(RateLimitConfigError::InvalidRequestsPerSecond(_))
        ));
        assert!(matches!(
            RateLimiterConfig::new(f64::NAN, 5),
            Err(RateLimitConfigError::InvalidRequestsPerSecond(_))
        ));
        assert!(matches!(
            RateLimiterConfig::new(f64::INFINITY, 5),
            Err(RateLimitConfigError::InvalidRequestsPerSecond(_))
        ));
        assert!(matches!(
            RateLimiterConfig::new(f64::NEG_INFINITY, 5),
            Err(RateLimitConfigError::InvalidRequestsPerSecond(_))
        ));
        assert!(matches!(
            RateLimiterConfig::new(f64::MIN_POSITIVE, 5),
            Err(RateLimitConfigError::InvalidRequestsPerSecond(_))
        ));
        assert_eq!(
            RateLimiterConfig::new(10.0, 0),
            Err(RateLimitConfigError::InvalidBurstSize)
        );

        // Struct literal bypassing `new` must still be rejected by `validate` and reader constructor.
        let invalid_literal = RateLimiterConfig {
            requests_per_second: 0.0,
            burst_size: 0,
        };
        assert!(invalid_literal.validate().is_err());
        assert!(RateLimitedLedgerReader::new(MockLedgerReader::new(), invalid_literal).is_err());

        let valid = RateLimiterConfig::new(0.5, 1).expect("fractional RPS should be valid");
        assert_eq!(valid.requests_per_second, 0.5);
        assert_eq!(valid.burst_size, 1);

        let default_cfg = RateLimiterConfig::default();
        assert!(default_cfg.validate().is_ok());
    }

    /// Helper asserting a duration falls within `[expected, expected + max_ticks * 1ms]`
    /// to account for Tokio's 1ms paused-timer wheel granularity.
    fn assert_paced_within(actual: Duration, expected: Duration, max_ticks: u64) {
        let tolerance = Duration::from_millis(max_ticks);
        assert!(
            actual >= expected && actual <= expected + tolerance,
            "expected duration in [{expected:?}, {:?}], got {actual:?}",
            expected + tolerance
        );
    }

    fn assert_paced_at(actual: Duration, expected: Duration) {
        assert_paced_within(actual, expected, 1);
    }

    #[tokio::test]
    async fn test_burst_allowance_and_sustained_rps_pacing() {
        tokio::time::pause();

        let mock = MockLedgerReader::new().with_latest_ledger(5_500_000, 22);
        let config = RateLimiterConfig::new(2.0, 3).unwrap();
        let reader = RateLimitedLedgerReader::new(mock, config).unwrap();

        let start = Instant::now();
        let mut timestamps = Vec::new();
        for _ in 0..6 {
            let info = reader.get_latest_ledger().await.unwrap();
            assert_eq!(info.sequence, 5_500_000);
            timestamps.push(start.elapsed());
        }

        // First 3 requests use the initial burst immediately; next 3 are paced at 2 RPS (500ms each).
        assert_eq!(timestamps[0], Duration::ZERO);
        assert_eq!(timestamps[1], Duration::ZERO);
        assert_eq!(timestamps[2], Duration::ZERO);
        assert_paced_at(timestamps[3], Duration::from_millis(500));
        assert_paced_at(timestamps[4], Duration::from_millis(1_000));
        assert_paced_at(timestamps[5], Duration::from_millis(1_500));
        // Sustained pacing has zero cumulative drift across consecutive paced calls.
        assert_eq!(timestamps[4] - timestamps[3], Duration::from_millis(500));
        assert_eq!(timestamps[5] - timestamps[4], Duration::from_millis(500));
    }

    #[tokio::test]
    async fn test_get_ledger_entries_gates_per_rpc_batch_call() {
        tokio::time::pause();

        let entries: Vec<RawLedgerEntry> = (0..5)
            .map(|i| {
                RawLedgerEntry::new(format!("key_{i}"), format!("val_{i}"), 6_000_000, 5_000_000)
            })
            .collect();
        let keys: Vec<String> = (0..5).map(|i| format!("key_{i}")).collect();

        let mock = MockLedgerReader::new().with_entries(entries);
        let config = RateLimiterConfig::new(2.0, 2).unwrap();
        let reader = RateLimitedLedgerReader::new(mock, config).unwrap();

        let start = Instant::now();

        // Empty query should not consume a token or block.
        let empty = reader.get_ledger_entries(&[]).await.unwrap();
        assert!(empty.is_empty());
        assert_eq!(start.elapsed(), Duration::ZERO);
        assert_eq!(reader.available_tokens().await, 2.0);

        // Two multi-key batch calls consume 1 token each (exhausting burst=2) immediately.
        let batch1 = reader.get_ledger_entries(&keys).await.unwrap();
        assert_eq!(batch1.len(), 5);
        assert_eq!(start.elapsed(), Duration::ZERO);

        let batch2 = reader.get_ledger_entries(&keys).await.unwrap();
        assert_eq!(batch2.len(), 5);
        assert_eq!(start.elapsed(), Duration::ZERO);

        // Third batch call waits 500ms (1 / 2.0 RPS), proving per-batch rather than per-key pacing.
        let batch3 = reader.get_ledger_entries(&keys).await.unwrap();
        assert_eq!(batch3.len(), 5);
        assert_paced_at(start.elapsed(), Duration::from_millis(500));
    }

    #[tokio::test]
    async fn test_shared_bucket_across_latest_ledger_and_entries() {
        tokio::time::pause();

        let entry = RawLedgerEntry::new("k1", "v1", 6_000_000, 5_000_000);
        let mock = MockLedgerReader::new()
            .with_latest_ledger(5_000_000, 22)
            .with_entry(entry);
        let config = RateLimiterConfig::new(4.0, 2).unwrap(); // 250ms per token
        let reader = RateLimitedLedgerReader::new(mock, config).unwrap();

        let start = Instant::now();
        reader.get_latest_ledger().await.unwrap();
        assert_eq!(start.elapsed(), Duration::ZERO);

        reader
            .get_ledger_entries(&["k1".to_string()])
            .await
            .unwrap();
        assert_eq!(start.elapsed(), Duration::ZERO);

        // Burst exhausted by the two mixed calls; subsequent calls are paced at 250ms intervals.
        reader.get_latest_ledger().await.unwrap();
        assert_paced_at(start.elapsed(), Duration::from_millis(250));

        reader
            .get_ledger_entries(&["k1".to_string()])
            .await
            .unwrap();
        assert_paced_at(start.elapsed(), Duration::from_millis(500));
    }

    #[tokio::test]
    async fn test_idle_refill_caps_at_burst_size() {
        tokio::time::pause();

        let mock = MockLedgerReader::new().with_latest_ledger(5_000_000, 22);
        let config = RateLimiterConfig::new(10.0, 3).unwrap(); // 100ms per token
        let reader = RateLimitedLedgerReader::new(mock, config).unwrap();

        // Drain the entire initial burst.
        for _ in 0..3 {
            reader.get_latest_ledger().await.unwrap();
        }
        assert_eq!(reader.available_tokens().await, 0.0);

        // Advance well beyond full refill time; tokens must cap at burst_size (3).
        tokio::time::advance(Duration::from_secs(10)).await;
        assert_eq!(reader.available_tokens().await, 3.0);

        let after_idle = Instant::now();
        for _ in 0..3 {
            reader.get_latest_ledger().await.unwrap();
        }
        assert_eq!(after_idle.elapsed(), Duration::ZERO);

        // 4th call must wait for 1 token (100ms).
        reader.get_latest_ledger().await.unwrap();
        assert_paced_at(after_idle.elapsed(), Duration::from_millis(100));
    }

    #[tokio::test]
    async fn test_partial_refill_waits_only_for_remaining_deficit() {
        tokio::time::pause();

        let mock = MockLedgerReader::new().with_latest_ledger(5_000_000, 22);
        let config = RateLimiterConfig::new(2.0, 1).unwrap(); // 500ms per token
        let reader = RateLimitedLedgerReader::new(mock, config).unwrap();

        let start = Instant::now();
        reader.get_latest_ledger().await.unwrap();
        assert_eq!(start.elapsed(), Duration::ZERO);

        // Advance 200ms -> 0.4 tokens accumulated, only 300ms remaining for a full token.
        tokio::time::advance(Duration::from_millis(200)).await;
        reader.get_latest_ledger().await.unwrap();
        assert_paced_at(start.elapsed(), Duration::from_millis(500));
    }

    #[tokio::test]
    async fn test_fractional_and_non_integer_rps_pacing() {
        tokio::time::pause();

        // Sub-1 RPS: 0.5 RPS => 1 request every 2 seconds.
        let slow_mock = MockLedgerReader::new().with_latest_ledger(5_000_000, 22);
        let slow_reader =
            RateLimitedLedgerReader::new(slow_mock, RateLimiterConfig::new(0.5, 1).unwrap())
                .unwrap();

        let start = Instant::now();
        slow_reader.get_latest_ledger().await.unwrap();
        assert_eq!(start.elapsed(), Duration::ZERO);
        slow_reader.get_latest_ledger().await.unwrap();
        assert_paced_within(start.elapsed(), Duration::from_secs(2), 1);
        slow_reader.get_latest_ledger().await.unwrap();
        assert_paced_within(start.elapsed(), Duration::from_secs(4), 2);

        // Non-terminating decimal interval: 3 RPS => 3 post-burst requests must take >= 1s.
        let odd_mock = MockLedgerReader::new().with_latest_ledger(5_000_000, 22);
        let odd_reader =
            RateLimitedLedgerReader::new(odd_mock, RateLimiterConfig::new(3.0, 1).unwrap())
                .unwrap();

        let odd_start = Instant::now();
        odd_reader.get_latest_ledger().await.unwrap(); // burst
        for _ in 0..3 {
            odd_reader.get_latest_ledger().await.unwrap();
        }
        assert_paced_within(odd_start.elapsed(), Duration::from_secs(1), 50);
    }

    #[tokio::test]
    async fn test_concurrent_callers_share_bucket_without_starvation_or_panic() {
        tokio::time::pause();

        let entry = RawLedgerEntry::new("k1", "v1", 6_000_000, 5_000_000);
        let mock = MockLedgerReader::new()
            .with_latest_ledger(5_500_000, 22)
            .with_entry(entry);
        // 5 RPS => 200ms per token, burst of 3.
        let config = RateLimiterConfig::new(5.0, 3).unwrap();
        let reader = Arc::new(RateLimitedLedgerReader::new(mock, config).unwrap());

        let start = Instant::now();
        let mut handles = Vec::new();
        for i in 0..10 {
            let reader = Arc::clone(&reader);
            handles.push(tokio::spawn(async move {
                if i % 2 == 0 {
                    let info = reader.get_latest_ledger().await.unwrap();
                    assert_eq!(info.sequence, 5_500_000);
                } else {
                    let res = reader
                        .get_ledger_entries(&["k1".to_string()])
                        .await
                        .unwrap();
                    assert_eq!(res.len(), 1);
                }
                start.elapsed()
            }));
        }

        let mut completion_times = Vec::with_capacity(handles.len());
        for handle in handles {
            completion_times.push(handle.await.expect("caller task should not panic"));
        }
        completion_times.sort();

        // First 3 burst calls finish immediately.
        assert_eq!(completion_times[0], Duration::ZERO);
        assert_eq!(completion_times[1], Duration::ZERO);
        assert_eq!(completion_times[2], Duration::ZERO);

        // Remaining 7 calls are paced at 200ms intervals without starvation or cumulative drift.
        for step in 1..=7 {
            assert_paced_at(
                completion_times[2 + step],
                Duration::from_millis(200 * step as u64),
            );
            if step > 1 {
                assert_eq!(
                    completion_times[2 + step] - completion_times[1 + step],
                    Duration::from_millis(200)
                );
            }
        }
    }

    #[tokio::test]
    async fn test_cancelled_waiter_does_not_lose_token_or_starve_next_caller() {
        tokio::time::pause();

        let mock = MockLedgerReader::new().with_latest_ledger(5_500_000, 22);
        let config = RateLimiterConfig::new(2.0, 1).unwrap(); // 500ms per token
        let reader = Arc::new(RateLimitedLedgerReader::new(mock, config).unwrap());

        let start = Instant::now();
        reader.get_latest_ledger().await.unwrap();
        assert_eq!(start.elapsed(), Duration::ZERO);

        let reader_a = Arc::clone(&reader);
        let waiter_a = tokio::spawn(async move {
            tokio::time::timeout(Duration::from_millis(200), reader_a.get_latest_ledger()).await
        });

        // Yield so waiter_a enters the mutex sleep before waiter_b queues behind it.
        tokio::task::yield_now().await;

        let reader_b = Arc::clone(&reader);
        let waiter_b = tokio::spawn(async move {
            let info = reader_b.get_latest_ledger().await.unwrap();
            assert_eq!(info.sequence, 5_500_000);
            start.elapsed()
        });

        assert!(waiter_a.await.unwrap().is_err(), "waiter_a should time out");
        let b_elapsed = waiter_b.await.unwrap();
        assert_paced_at(b_elapsed, Duration::from_millis(500));
    }

    #[tokio::test]
    async fn test_cloned_reader_shares_bucket_and_propagates_errors() {
        tokio::time::pause();

        let mock = MockLedgerReader::new()
            .with_latest_ledger(5_500_000, 22)
            .with_velocity(5.2);
        let config = RateLimiterConfig::new(2.0, 1).unwrap();
        let reader_a = RateLimitedLedgerReader::new(mock, config).unwrap();
        let reader_b = reader_a.clone();

        assert_eq!(reader_a.config(), config);
        assert_eq!(
            reader_a.get_ledger_close_velocity(5_500_000).await.unwrap(),
            Some(5.2)
        );

        let start = Instant::now();
        reader_a.get_latest_ledger().await.unwrap();
        assert_eq!(start.elapsed(), Duration::ZERO);

        // Clone shares the same bucket so it must wait 500ms.
        reader_b.get_latest_ledger().await.unwrap();
        assert_paced_at(start.elapsed(), Duration::from_millis(500));

        // Inner errors are propagated cleanly without panicking.
        let failing_mock = MockLedgerReader::new().with_error(SentinelError::RpcConnectionTimeout(
            "simulated timeout".to_string(),
        ));
        let failing_reader = RateLimitedLedgerReader::new(failing_mock, config).unwrap();
        assert!(matches!(
            failing_reader.get_latest_ledger().await,
            Err(SentinelError::RpcConnectionTimeout(_))
        ));
        assert!(matches!(
            failing_reader.get_ledger_entries(&["k1".to_string()]).await,
            Err(SentinelError::RpcConnectionTimeout(_))
        ));
        let _inner = failing_reader.into_inner();
    }
}
