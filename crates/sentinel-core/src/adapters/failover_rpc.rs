//! Deterministic, health-aware failover across prioritized ledger readers.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;

use crate::adapters::SorobanRpcClient;
use crate::error::{SentinelError, SentinelResult};
use crate::ports::reader::{LatestLedgerInfo, LedgerReader, RawLedgerEntry, ReaderHealth};

/// Configuration for endpoint health tracking and recovery probes.
#[derive(Debug, Clone)]
pub struct FailoverConfig {
    /// Delay between health probes for degraded and unhealthy endpoints.
    pub probe_interval: Duration,
    /// Successful calls above this latency classify the endpoint as degraded.
    pub high_latency_threshold: Duration,
    /// Half-life used to decay each endpoint's accumulated transient error score.
    pub error_half_life: Duration,
}

impl Default for FailoverConfig {
    fn default() -> Self {
        Self {
            probe_interval: Duration::from_secs(30),
            high_latency_threshold: Duration::from_secs(2),
            error_half_life: Duration::from_secs(60),
        }
    }
}

/// Read-only health information for one prioritized endpoint.
#[derive(Debug, Clone)]
pub struct EndpointHealthSnapshot {
    /// Endpoint URL or caller-provided name.
    pub endpoint: String,
    /// Current health classification.
    pub health: ReaderHealth,
    /// Number of consecutive retryable failures.
    pub consecutive_errors: u32,
    /// Exponentially-decayed transient error score.
    pub error_score: f64,
    /// Exponentially weighted moving average of successful call latency.
    pub latency: Option<Duration>,
}

struct Endpoint {
    name: String,
    reader: Arc<dyn LedgerReader>,
}

struct EndpointState {
    health: ReaderHealth,
    consecutive_errors: u32,
    error_score: f64,
    last_score_update: Instant,
    next_probe: Instant,
    latency_ewma: Option<Duration>,
}

impl EndpointState {
    fn new() -> Self {
        let now = Instant::now();
        Self {
            health: ReaderHealth::Healthy,
            consecutive_errors: 0,
            error_score: 0.0,
            last_score_update: now,
            next_probe: now,
            latency_ewma: None,
        }
    }

    fn decay_error_score(&mut self, now: Instant, half_life: Duration) {
        let elapsed = now.saturating_duration_since(self.last_score_update);
        self.error_score = if half_life.is_zero() {
            0.0
        } else {
            self.error_score * 2.0_f64.powf(-elapsed.as_secs_f64() / half_life.as_secs_f64())
        };
        self.last_score_update = now;
        if self.health == ReaderHealth::Unhealthy && self.error_score < 0.25 {
            self.health = ReaderHealth::Degraded;
        }
    }

    fn record_success(&mut self, latency: Duration, now: Instant, config: &FailoverConfig) {
        self.decay_error_score(now, config.error_half_life);
        self.consecutive_errors = 0;
        self.latency_ewma = Some(match self.latency_ewma {
            Some(previous) => {
                let blended = previous.as_secs_f64() * 0.8 + latency.as_secs_f64() * 0.2;
                Duration::from_secs_f64(blended)
            }
            None => latency,
        });
        self.health = if latency > config.high_latency_threshold
            || self.latency_ewma.unwrap() > config.high_latency_threshold
        {
            ReaderHealth::Degraded
        } else {
            ReaderHealth::Healthy
        };
        self.next_probe = now + config.probe_interval;
    }

    fn record_failure(&mut self, now: Instant, config: &FailoverConfig) {
        self.decay_error_score(now, config.error_half_life);
        self.consecutive_errors = self.consecutive_errors.saturating_add(1);
        self.error_score += 1.0;
        self.health = ReaderHealth::Unhealthy;
        self.next_probe = now + config.probe_interval;
    }
}

/// Tries prioritized Soroban RPC readers in order and fails over on transient errors.
///
/// The first supplied endpoint has the highest priority. Endpoints that become
/// degraded or unhealthy are periodically probed with `get_latest_ledger` and
/// rejoin the healthy pool after a successful, sufficiently fast response.
pub struct MultiRpcFailoverReader {
    endpoints: Vec<Endpoint>,
    states: Mutex<Vec<EndpointState>>,
    config: FailoverConfig,
}

impl MultiRpcFailoverReader {
    /// Creates a failover reader from prioritized Soroban RPC clients.
    pub fn new(clients: Vec<SorobanRpcClient>) -> Self {
        let endpoints = clients
            .into_iter()
            .map(|client| {
                let name = client.rpc_url().to_string();
                Endpoint {
                    name,
                    reader: Arc::new(client),
                }
            })
            .collect();
        Self::from_endpoints(endpoints, FailoverConfig::default())
    }

    /// Creates a failover reader with custom health thresholds.
    pub fn with_config(clients: Vec<SorobanRpcClient>, config: FailoverConfig) -> Self {
        let endpoints = clients
            .into_iter()
            .map(|client| {
                let name = client.rpc_url().to_string();
                Endpoint {
                    name,
                    reader: Arc::new(client),
                }
            })
            .collect();
        Self::from_endpoints(endpoints, config)
    }

    /// Creates a failover reader from prioritized `LedgerReader` implementations.
    ///
    /// Names are included in health snapshots. This constructor is useful for
    /// embedding custom readers alongside RPC clients.
    pub fn from_readers(
        readers: Vec<(String, Arc<dyn LedgerReader>)>,
        config: FailoverConfig,
    ) -> Self {
        Self::from_endpoints(
            readers
                .into_iter()
                .map(|(name, reader)| Endpoint { name, reader })
                .collect(),
            config,
        )
    }

    fn from_endpoints(endpoints: Vec<Endpoint>, config: FailoverConfig) -> Self {
        let states = (0..endpoints.len()).map(|_| EndpointState::new()).collect();
        Self {
            endpoints,
            states: Mutex::new(states),
            config,
        }
    }

    /// Returns current endpoint health in priority order.
    pub fn health(&self) -> Vec<EndpointHealthSnapshot> {
        let now = Instant::now();
        let mut states = self.states.lock().unwrap_or_else(|e| e.into_inner());
        for state in states.iter_mut() {
            state.decay_error_score(now, self.config.error_half_life);
        }
        self.endpoints
            .iter()
            .zip(states.iter())
            .map(|(endpoint, state)| EndpointHealthSnapshot {
                endpoint: endpoint.name.clone(),
                health: state.health,
                consecutive_errors: state.consecutive_errors,
                error_score: state.error_score,
                latency: state.latency_ewma,
            })
            .collect()
    }

    async fn probe_due_endpoints(&self) {
        let now = Instant::now();
        let due: Vec<usize> = {
            let mut states = self.states.lock().unwrap_or_else(|e| e.into_inner());
            states
                .iter_mut()
                .enumerate()
                .filter_map(|(index, state)| {
                    if state.health != ReaderHealth::Healthy && now >= state.next_probe {
                        state.next_probe = now + self.config.probe_interval;
                        Some(index)
                    } else {
                        None
                    }
                })
                .collect()
        };

        for index in due {
            let start = Instant::now();
            let result = self.endpoints[index].reader.get_latest_ledger().await;
            let elapsed = start.elapsed();
            let mut states = self.states.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(state) = states.get_mut(index) {
                match result {
                    Ok(_) => state.record_success(elapsed, Instant::now(), &self.config),
                    Err(error) if is_retryable(&error) => {
                        state.record_failure(Instant::now(), &self.config)
                    }
                    // A non-transient RPC/application error proves the endpoint is reachable.
                    Err(_) => state.record_success(elapsed, Instant::now(), &self.config),
                }
            }
        }
    }

    async fn execute<T, F>(&self, operation: F) -> SentinelResult<T>
    where
        T: Send,
        F: Fn(Arc<dyn LedgerReader>) -> Pin<Box<dyn Future<Output = SentinelResult<T>> + Send>>
            + Sync,
    {
        if self.endpoints.is_empty() {
            return Err(SentinelError::RpcConnectionTimeout(
                "No RPC endpoints configured".to_string(),
            ));
        }

        self.probe_due_endpoints().await;

        let order: Vec<usize> = {
            let mut states = self.states.lock().unwrap_or_else(|e| e.into_inner());
            let now = Instant::now();
            for state in states.iter_mut() {
                state.decay_error_score(now, self.config.error_half_life);
            }
            let mut indices: Vec<usize> = (0..states.len()).collect();
            indices.sort_by_key(|index| match states[*index].health {
                ReaderHealth::Healthy => 0,
                ReaderHealth::Degraded => 1,
                ReaderHealth::Unhealthy => 2,
            });
            indices
        };

        let mut last_error = None;
        for index in order {
            let start = Instant::now();
            let result = operation(Arc::clone(&self.endpoints[index].reader)).await;
            let elapsed = start.elapsed();
            match result {
                Ok(value) => {
                    if let Some(state) = self
                        .states
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .get_mut(index)
                    {
                        state.record_success(elapsed, Instant::now(), &self.config);
                    }
                    return Ok(value);
                }
                Err(error) if is_retryable(&error) => {
                    if let Some(state) = self
                        .states
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .get_mut(index)
                    {
                        state.record_failure(Instant::now(), &self.config);
                    }
                    last_error = Some(error);
                }
                Err(error) => return Err(error),
            }
        }

        Err(last_error.unwrap_or_else(|| {
            SentinelError::RpcConnectionTimeout("No RPC endpoints available".to_string())
        }))
    }
}

fn is_retryable(error: &SentinelError) -> bool {
    matches!(
        error,
        SentinelError::RpcConnectionTimeout(_)
            | SentinelError::RpcResponseError {
                code: 429 | 503,
                ..
            }
    )
}

#[async_trait]
impl LedgerReader for MultiRpcFailoverReader {
    async fn get_latest_ledger(&self) -> SentinelResult<LatestLedgerInfo> {
        self.execute(|reader| Box::pin(async move { reader.get_latest_ledger().await }))
            .await
    }

    async fn get_ledger_entries(
        &self,
        keys_xdr_base64: &[String],
    ) -> SentinelResult<Vec<RawLedgerEntry>> {
        let keys = keys_xdr_base64.to_vec();
        self.execute(move |reader| {
            let keys = keys.clone();
            Box::pin(async move { reader.get_ledger_entries(&keys).await })
        })
        .await
    }

    async fn get_ledger_close_velocity(&self, latest_ledger: u32) -> SentinelResult<Option<f64>> {
        self.execute(move |reader| {
            Box::pin(async move { reader.get_ledger_close_velocity(latest_ledger).await })
        })
        .await
    }

    async fn get_ledger_close_samples(
        &self,
        latest_ledger: u32,
        sample_window: u32,
    ) -> SentinelResult<Option<Vec<crate::domain::burn_rate::LedgerCloseSample>>> {
        self.execute(move |reader| {
            Box::pin(async move {
                reader
                    .get_ledger_close_samples(latest_ledger, sample_window)
                    .await
            })
        })
        .await
    }
}
