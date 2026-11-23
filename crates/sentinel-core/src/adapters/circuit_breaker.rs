//! In-memory circuit breaker for a single ledger reader.

use std::{
    future::Future,
    pin::Pin,
    sync::Mutex,
    time::{Duration, Instant},
};

use crate::{
    error::{SentinelError, SentinelResult},
    ports::{LatestLedgerInfo, LedgerReader, RawLedgerEntry},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitState {
    Closed,
    Open,
    HalfOpen,
}

#[derive(Debug, Clone, Copy)]
pub struct CircuitBreakerConfig {
    pub failure_threshold: u32,
    pub reset_timeout: Duration,
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            failure_threshold: 5,
            reset_timeout: Duration::from_secs(30),
        }
    }
}

struct InnerState {
    state: CircuitState,
    consecutive_failures: u32,
    opened_at: Option<Instant>,
    probe_in_flight: bool,
    generation: u64,
}

#[derive(Clone, Copy)]
struct CallPermit {
    generation: u64,
    half_open_probe: bool,
}

struct ProbeReset<'a> {
    state: &'a Mutex<InnerState>,
    permit: CallPermit,
}

impl Drop for ProbeReset<'_> {
    fn drop(&mut self) {
        if !self.permit.half_open_probe {
            return;
        }
        if let Ok(mut state) = self.state.lock() {
            if state.generation == self.permit.generation && state.state == CircuitState::HalfOpen {
                state.probe_in_flight = false;
            }
        }
    }
}

/// Wraps a ledger reader and fails fast during repeated RPC outages.
pub struct CircuitBreakerReader<R> {
    reader: R,
    config: CircuitBreakerConfig,
    state: Mutex<InnerState>,
}

impl<R> CircuitBreakerReader<R> {
    pub fn new(reader: R) -> Self {
        Self::with_config(reader, CircuitBreakerConfig::default())
    }

    pub fn with_config(reader: R, config: CircuitBreakerConfig) -> Self {
        Self {
            reader,
            config: CircuitBreakerConfig {
                failure_threshold: config.failure_threshold.max(1),
                ..config
            },
            state: Mutex::new(InnerState {
                state: CircuitState::Closed,
                consecutive_failures: 0,
                opened_at: None,
                probe_in_flight: false,
                generation: 0,
            }),
        }
    }

    pub fn state(&self) -> CircuitState {
        let mut state = self.state.lock().expect("circuit breaker mutex poisoned");
        if state.state == CircuitState::Open
            && state
                .opened_at
                .is_some_and(|at| at.elapsed() >= self.config.reset_timeout)
        {
            state.state = CircuitState::HalfOpen;
            state.probe_in_flight = false;
        }
        state.state
    }

    async fn call<T, F>(&self, operation: F) -> SentinelResult<T>
    where
        F: for<'a> FnOnce(&'a R) -> Pin<Box<dyn Future<Output = SentinelResult<T>> + Send + 'a>>,
    {
        let permit = self.begin_call()?;
        let _probe_reset = ProbeReset {
            state: &self.state,
            permit,
        };
        let result = operation(&self.reader).await;
        self.finish_call(permit, &result);
        result
    }

    fn begin_call(&self) -> SentinelResult<CallPermit> {
        let (generation, half_open_probe) = {
            let mut state = self.state.lock().expect("circuit breaker mutex poisoned");
            if state.state == CircuitState::Open
                && state
                    .opened_at
                    .is_some_and(|at| at.elapsed() >= self.config.reset_timeout)
            {
                state.state = CircuitState::HalfOpen;
                state.probe_in_flight = false;
            }
            let half_open_probe = match state.state {
                CircuitState::Open => return Err(SentinelError::CircuitBreakerOpen),
                CircuitState::HalfOpen if state.probe_in_flight => {
                    return Err(SentinelError::CircuitBreakerOpen)
                }
                CircuitState::HalfOpen => {
                    state.probe_in_flight = true;
                    true
                }
                CircuitState::Closed => false,
            };
            (state.generation, half_open_probe)
        };
        Ok(CallPermit {
            generation,
            half_open_probe,
        })
    }

    fn finish_call<T>(&self, permit: CallPermit, result: &SentinelResult<T>) {
        let mut state = self.state.lock().expect("circuit breaker mutex poisoned");
        if permit.generation != state.generation {
            return;
        }
        match result {
            Ok(_) => {
                state.state = CircuitState::Closed;
                state.consecutive_failures = 0;
                state.opened_at = None;
                state.probe_in_flight = false;
            }
            Err(error) => {
                let was_probe = state.state == CircuitState::HalfOpen;
                state.probe_in_flight = false;
                if was_probe || is_trip_failure(error) {
                    state.consecutive_failures = if was_probe {
                        self.config.failure_threshold
                    } else {
                        state.consecutive_failures.saturating_add(1)
                    };
                    if state.consecutive_failures >= self.config.failure_threshold {
                        state.state = CircuitState::Open;
                        state.opened_at = Some(Instant::now());
                        state.generation = state.generation.wrapping_add(1);
                    }
                } else {
                    state.consecutive_failures = 0;
                }
            }
        }
    }
}

fn is_trip_failure(error: &SentinelError) -> bool {
    matches!(
        error,
        SentinelError::RpcConnectionTimeout(_) | SentinelError::RpcResponseError { code: 503, .. }
    )
}

#[async_trait::async_trait]
impl<R: LedgerReader> LedgerReader for CircuitBreakerReader<R> {
    async fn get_latest_ledger(&self) -> SentinelResult<LatestLedgerInfo> {
        self.call(|r| Box::pin(r.get_latest_ledger())).await
    }
    async fn get_ledger_entries(&self, keys: &[String]) -> SentinelResult<Vec<RawLedgerEntry>> {
        let permit = self.begin_call()?;
        let _probe_reset = ProbeReset {
            state: &self.state,
            permit,
        };
        let result = self.reader.get_ledger_entries(keys).await;
        self.finish_call(permit, &result);
        result
    }
    async fn get_ledger_close_velocity(&self, latest: u32) -> SentinelResult<Option<f64>> {
        self.call(|r| Box::pin(r.get_ledger_close_velocity(latest)))
            .await
    }
}
