use async_trait::async_trait;
use sentinel_core::{
    CircuitBreakerConfig, CircuitBreakerReader, CircuitState, LatestLedgerInfo, LedgerReader,
    RawLedgerEntry, SentinelError, SentinelResult,
};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

struct MockReader {
    calls: Arc<AtomicUsize>,
    failures_left: std::sync::Mutex<usize>,
}
#[async_trait]
impl LedgerReader for MockReader {
    async fn get_latest_ledger(&self) -> SentinelResult<LatestLedgerInfo> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let mut remaining = self.failures_left.lock().unwrap();
        if *remaining > 0 {
            *remaining -= 1;
            Err(SentinelError::RpcResponseError {
                code: 503,
                message: "unavailable".into(),
            })
        } else {
            Ok(LatestLedgerInfo::new(10, 22))
        }
    }
    async fn get_ledger_entries(&self, _: &[String]) -> SentinelResult<Vec<RawLedgerEntry>> {
        Ok(Vec::new())
    }
}

#[tokio::test]
async fn opens_after_threshold_and_fails_fast_until_cooldown() {
    let calls = Arc::new(AtomicUsize::new(0));
    let reader = MockReader {
        calls: calls.clone(),
        failures_left: std::sync::Mutex::new(2),
    };
    let breaker = CircuitBreakerReader::with_config(
        reader,
        CircuitBreakerConfig {
            failure_threshold: 2,
            reset_timeout: std::time::Duration::from_millis(40),
        },
    );
    assert!(breaker.get_latest_ledger().await.is_err());
    assert!(breaker.get_latest_ledger().await.is_err());
    assert_eq!(breaker.state(), CircuitState::Open);
    assert_eq!(
        breaker.get_latest_ledger().await.unwrap_err(),
        SentinelError::CircuitBreakerOpen
    );
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert_eq!(breaker.state(), CircuitState::HalfOpen);
    assert_eq!(breaker.get_latest_ledger().await.unwrap().sequence, 10);
    assert_eq!(breaker.state(), CircuitState::Closed);
}

#[tokio::test]
async fn only_http_503_and_timeout_trip_the_breaker() {
    struct BadRequestReader;
    #[async_trait]
    impl LedgerReader for BadRequestReader {
        async fn get_latest_ledger(&self) -> SentinelResult<LatestLedgerInfo> {
            Err(SentinelError::RpcResponseError {
                code: 400,
                message: "bad request".into(),
            })
        }
        async fn get_ledger_entries(&self, _: &[String]) -> SentinelResult<Vec<RawLedgerEntry>> {
            Ok(Vec::new())
        }
    }
    let breaker = CircuitBreakerReader::with_config(
        BadRequestReader,
        CircuitBreakerConfig {
            failure_threshold: 1,
            reset_timeout: std::time::Duration::from_secs(1),
        },
    );
    assert!(breaker.get_latest_ledger().await.is_err());
    assert_eq!(breaker.state(), CircuitState::Closed);
}
