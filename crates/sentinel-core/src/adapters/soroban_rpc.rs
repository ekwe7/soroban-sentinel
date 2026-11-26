//! Soroban JSON-RPC 2.0 client adapter implementing [`LedgerReader`].

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};
use stellar_xdr::curr::{ReadXdr, ScVal, SorobanTransactionData};
use tracing::{debug, info};

use crate::adapters::xdr_keys::{parse_contract_id, safe_xdr_limits};
use crate::domain::events::{ContractEvent, DecodedContractEvent, SorobanEventType};

use crate::domain::burn_rate::{
    calculate_burn_rate, LedgerCloseSample, DEFAULT_SAMPLE_WINDOW_LEDGERS,
};
use crate::domain::cache::{footprint_cache_key, SimulationCache, SimulationCacheEntry};
use crate::domain::FeePercentiles;
use crate::domain::TARGET_LEDGER_CLOSE_SECONDS;
use crate::error::{SentinelError, SentinelResult};
use crate::ports::reader::{LatestLedgerInfo, LedgerReader, RawLedgerEntry};
use crate::ports::simulator::SimulationEngine;
use crate::ports::EventStreamReader;

/// Maximum number of keys allowed in a single `getLedgerEntries` request by Soroban RPC.
pub const MAX_KEYS_PER_REQUEST: usize = 200;

/// Maximum number of events requested per `getEvents` page.
pub const MAX_EVENTS_PER_PAGE: u32 = 100;

/// Safety bound on `getEvents` pagination loops to prevent unbounded reads.
pub const MAX_EVENT_PAGES: usize = 100;
/// Maximum number of JSON-RPC calls sent in one HTTP batch.
pub const MAX_BATCH_REQUESTS: usize = 20;

/// Default maximum retry attempts for transient errors (HTTP 429 / 503).
pub const DEFAULT_MAX_RETRIES: u32 = 3;

/// Default initial retry delay in milliseconds.
pub const DEFAULT_BASE_RETRY_DELAY_MS: u64 = 100;

/// Default maximum backoff delay ceiling in seconds.
pub const DEFAULT_MAX_DELAY_SECS: u64 = 30;

/// Jitter mode applied to exponential backoff delays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Jitter {
    /// No jitter; exact computed backoff or clamped Retry-After delay.
    None,
    /// Full jitter: uniform random delay between 0 and backoff.
    Full,
    /// Equal jitter: half constant backoff + half uniform random.
    Equal,
}

/// Retry configuration for handling transient HTTP and network errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetryPolicy {
    /// Maximum number of attempts before propagating the error.
    pub max_retries: u32,
    /// Base delay applied to exponential backoff calculation.
    pub base_delay: Duration,
    /// Upper bound cap for computed backoff or server-provided Retry-After.
    pub max_delay: Duration,
    /// Jitter strategy applied to delays.
    pub jitter: Jitter,
    /// Optional seed for deterministic PRNG in tests.
    pub seed: Option<u64>,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: DEFAULT_MAX_RETRIES,
            base_delay: Duration::from_millis(DEFAULT_BASE_RETRY_DELAY_MS),
            max_delay: Duration::from_secs(DEFAULT_MAX_DELAY_SECS),
            jitter: Jitter::Full,
            seed: None,
        }
    }
}

impl RetryPolicy {
    /// Set a custom max delay cap.
    pub fn with_max_delay(mut self, max_delay: Duration) -> Self {
        self.max_delay = max_delay;
        self
    }

    /// Set a custom jitter strategy.
    pub fn with_jitter(mut self, jitter: Jitter) -> Self {
        self.jitter = jitter;
        self
    }

    /// Set a seed for deterministic jitter in tests.
    pub fn with_seed(mut self, seed: u64) -> Self {
        self.seed = Some(seed);
        self
    }

    /// Computes the delay before the next retry attempt given current attempt count and optional Retry-After header.
    pub fn next_delay(&self, attempt: u32, retry_after: Option<Duration>) -> Duration {
        if let Some(ra) = retry_after {
            // Honour Retry-After delta-seconds clamped to max_delay
            return std::cmp::min(ra, self.max_delay);
        }

        // Exponential backoff: base_delay * 2^(attempt - 1)
        let exp_factor = 2u32.saturating_pow(attempt.saturating_sub(1));
        let computed = self.base_delay.saturating_mul(exp_factor);
        let capped = std::cmp::min(computed, self.max_delay);

        match self.jitter {
            Jitter::None => capped,
            Jitter::Full => {
                let factor = self.pseudo_random_factor(attempt);
                capped.mul_f64(factor)
            }
            Jitter::Equal => {
                let half = capped / 2;
                let factor = self.pseudo_random_factor(attempt);
                half + half.mul_f64(factor)
            }
        }
    }

    fn pseudo_random_factor(&self, attempt: u32) -> f64 {
        let seed = self.seed.unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(0x8543_291a_bcde_f012)
        });
        // SplitMix64 step
        let mut x = seed
            .wrapping_add(attempt as u64)
            .wrapping_mul(0x9E37_79B9_7F4A_7C15);
        x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        x ^= x >> 31;
        (x as f64) / (u64::MAX as f64)
    }
}

/// Maximum allowed RPC response body size in bytes (10 MB) to prevent unbounded memory exhaustion.
pub const MAX_RPC_RESPONSE_BYTES: usize = 10 * 1024 * 1024;

/// Validates that an RPC URL is syntactically valid and uses the http or https scheme.
pub fn validate_rpc_url(url: &str) -> SentinelResult<()> {
    let parsed = reqwest::Url::parse(url)
        .map_err(|e| SentinelError::InvalidRpcUrl(format!("'{}': {}", url, e)))?;
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return Err(SentinelError::InvalidRpcUrl(format!(
            "Unsupported scheme '{}'. Only 'http' and 'https' are supported.",
            parsed.scheme()
        )));
    }
    Ok(())
}

/// Returns true when a query parameter name or value looks like a secret.
fn is_likely_secret(s: &str) -> bool {
    let lower = s.to_ascii_lowercase();
    ["key", "secret", "token", "api"]
        .iter()
        .any(|needle| lower.contains(needle))
}

/// Returns true when a path segment looks like a Soroban contract/account address.
fn is_soroban_address(segment: &str) -> bool {
    segment.len() >= 5 && (segment.starts_with('G') || segment.starts_with('C'))
}

/// Redacts sensitive information from an RPC URL for safe logging.
///
/// - Removes userinfo (`username:password@`)
/// - Masks query parameter values whose name or value looks like a secret
/// - Replaces path segments that look like Soroban addresses with `redacted`
/// - Returns the original string verbatim when nothing needed redaction
pub fn redact_rpc_url(url: &str) -> String {
    let mut parsed = match reqwest::Url::parse(url) {
        Ok(u) => u,
        Err(_) => return url.to_string(),
    };

    let mut redacted = false;

    if !parsed.username().is_empty() || parsed.password().is_some() {
        let _ = parsed.set_username("");
        let _ = parsed.set_password(None);
        redacted = true;
    }

    if parsed.query().is_some() {
        let pairs: Vec<(String, String)> = parsed
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        let mut new_pairs = Vec::with_capacity(pairs.len());
        for (key, value) in pairs {
            if is_likely_secret(&key) || is_likely_secret(&value) {
                new_pairs.push((key, "redacted".to_string()));
                redacted = true;
            } else {
                new_pairs.push((key, value));
            }
        }
        let query = new_pairs
            .iter()
            .map(|(k, v)| format!("{}={}", k, v))
            .collect::<Vec<_>>()
            .join("&");
        parsed.set_query(Some(&query));
    }

    let path = parsed.path().to_string();
    let new_path = path
        .split('/')
        .map(|segment| {
            if is_soroban_address(segment) {
                redacted = true;
                "redacted".to_string()
            } else {
                segment.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("/");
    if redacted {
        parsed.set_path(&new_path);
    }

    if !redacted {
        return url.to_string();
    }

    parsed.to_string()
}

/// Parses standard HTTP Retry-After delta-seconds. Invalid values or HTTP-date formats return None.
fn parse_retry_after(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    let header_val = headers.get(reqwest::header::RETRY_AFTER)?;
    let val_str = header_val.to_str().ok()?.trim();
    if let Ok(secs) = val_str.parse::<u64>() {
        Some(Duration::from_secs(secs))
    } else {
        None
    }
}

/// Subset of a `simulateTransaction` JSON-RPC result used for fee caching.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SimulateTransactionResult {
    min_resource_fee: String,
    latest_ledger: u32,
    #[serde(default)]
    transaction_data: Option<String>,
}

/// JSON-RPC 2.0 client for interacting with Soroban RPC nodes.
#[derive(Debug, Clone)]
pub struct SorobanRpcClient {
    rpc_url: String,
    http_client: reqwest::Client,
    retry_policy: RetryPolicy,
    simulation_cache: Option<Arc<SimulationCache>>,
}

impl SorobanRpcClient {
    /// Create a new `SorobanRpcClient` with default timeout (10 seconds), 3-hop redirect limit, and standard retry policy.
    pub fn new(rpc_url: impl Into<String>) -> Self {
        Self::new_with_timeout(rpc_url, Duration::from_secs(10))
    }

    /// Create a new `SorobanRpcClient` with a configurable request timeout.
    pub fn new_with_timeout(rpc_url: impl Into<String>, timeout: Duration) -> Self {
        let http_client = reqwest::Client::builder()
            .timeout(timeout)
            .redirect(reqwest::redirect::Policy::limited(3))
            .gzip(true)
            .zstd(true)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        Self {
            rpc_url: rpc_url.into(),
            http_client,
            retry_policy: RetryPolicy::default(),
            simulation_cache: None,
        }
    }

    /// Set a custom request timeout.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.http_client = reqwest::Client::builder()
            .timeout(timeout)
            .redirect(reqwest::redirect::Policy::limited(3))
            .gzip(true)
            .zstd(true)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        self
    }

    /// Create a `SorobanRpcClient` with a custom pre-configured `reqwest::Client`.
    pub fn with_client(rpc_url: impl Into<String>, http_client: reqwest::Client) -> Self {
        Self {
            rpc_url: rpc_url.into(),
            http_client,
            retry_policy: RetryPolicy::default(),
            simulation_cache: None,
        }
    }

    /// Set a custom retry policy.
    pub fn with_retry_policy(mut self, retry_policy: RetryPolicy) -> Self {
        self.retry_policy = retry_policy;
        self
    }

    /// Returns the target RPC endpoint URL.
    pub fn rpc_url(&self) -> &str {
        &self.rpc_url
    }

    /// Returns the active retry policy.
    pub fn retry_policy(&self) -> &RetryPolicy {
        &self.retry_policy
    }

    pub async fn get_health(&self) -> SentinelResult<crate::ports::reader::RpcHealth> {
        self.send_rpc_request("getHealth", serde_json::json!({}))
            .await
    }

    pub async fn get_network(&self) -> SentinelResult<crate::ports::reader::RpcNetwork> {
        self.send_rpc_request("getNetwork", serde_json::json!({}))
            .await
    }

    /// Fetch Soroban inclusion fee percentiles from the network fee oracle.
    pub async fn get_fee_stats(&self) -> SentinelResult<FeePercentiles> {
        let result: serde_json::Value = self
            .send_rpc_request("getFeeStats", serde_json::json!({}))
            .await?;
        parse_fee_percentiles(&result)
    }

    /// Enables in-memory caching of simulation fee results for repeated footprints.
    ///
    /// Without this builder the client behaves exactly as before: every call to
    /// [`Self::simulate_transaction`] performs a JSON-RPC simulation.
    pub fn with_simulation_cache(mut self, cache: SimulationCache) -> Self {
        self.simulation_cache = Some(Arc::new(cache));
        self
    }

    /// Returns the configured simulation cache, if any.
    pub fn simulation_cache(&self) -> Option<&SimulationCache> {
        self.simulation_cache.as_deref()
    }

    /// Simulates a transaction and caches the fee result by footprint XDR hash.
    ///
    /// A cache hit never issues an RPC request. A miss, or an entry older than
    /// the cache's ledger horizon, triggers `simulateTransaction`. CPU
    /// instructions and memory bytes are taken from the returned
    /// `SorobanTransactionData` XDR; memory bytes are the sum of read and write
    /// bytes in the declared resource footprint.
    #[tracing::instrument(skip(self, transaction_xdr_base64, footprint_xdr_base64))]
    pub async fn simulate_transaction(
        &self,
        transaction_xdr_base64: &str,
        footprint_xdr_base64: &str,
        latest_ledger: u32,
    ) -> SentinelResult<SimulationCacheEntry> {
        let cache_key = footprint_cache_key(footprint_xdr_base64);
        if let Some(cache) = &self.simulation_cache {
            if let Some(cached) = cache.get(&cache_key, latest_ledger) {
                return Ok(cached);
            }
        }

        let result: SimulateTransactionResult = self
            .send_rpc_request(
                "simulateTransaction",
                serde_json::json!({ "transaction": transaction_xdr_base64 }),
            )
            .await?;

        let min_resource_fee = result.min_resource_fee.parse::<u64>().map_err(|error| {
            SentinelError::SerializationError(format!(
                "Invalid minResourceFee in simulateTransaction response: {error}"
            ))
        })?;

        let (cpu_instructions, memory_bytes) = match result.transaction_data {
            Some(transaction_data) => {
                let decoded = SorobanTransactionData::from_xdr_base64(
                    &transaction_data,
                    crate::adapters::safe_xdr_limits(),
                )
                .map_err(|error| {
                    SentinelError::SerializationError(format!(
                        "Invalid transactionData in simulateTransaction response: {error}"
                    ))
                })?;
                let resources = decoded.resources;
                (
                    u64::from(resources.instructions),
                    u64::from(resources.read_bytes)
                        .saturating_add(u64::from(resources.write_bytes)),
                )
            }
            None => (0, 0),
        };

        let entry = SimulationCacheEntry {
            min_resource_fee,
            latest_ledger: result.latest_ledger,
            cpu_instructions,
            memory_bytes,
        };
        if let Some(cache) = &self.simulation_cache {
            cache.insert(cache_key, entry.clone());
        }
        Ok(entry)
    }

    async fn send_rpc_payload<P: Serialize + Sync + ?Sized>(
        &self,
        payload: &P,
        retryable: bool,
    ) -> SentinelResult<Vec<u8>> {
        let redacted_url = redact_rpc_url(&self.rpc_url);
        let mut attempt: u32 = 0;
        loop {
            attempt += 1;
            let started_at = Instant::now();
            match self
                .http_client
                .post(&self.rpc_url)
                .header(reqwest::header::ACCEPT_ENCODING, "gzip, zstd")
                .json(payload)
                .send()
                .await
            {
                Ok(response) => {
                    let status = response.status();
                    info!(
                        rpc_url = %redacted_url,
                        attempt = attempt,
                        http_status = status.as_u16(),
                        latency_ms = started_at.elapsed().as_millis() as u64,
                        "rpc_request"
                    );

                    // Check for rate limiting (429) or transient server unavailability (503)
                    if retryable
                        && (status == reqwest::StatusCode::TOO_MANY_REQUESTS
                            || status == reqwest::StatusCode::SERVICE_UNAVAILABLE)
                        && attempt < self.retry_policy.max_retries
                    {
                        let retry_after = parse_retry_after(response.headers());
                        let backoff = self.retry_policy.next_delay(attempt, retry_after);
                        debug!(
                            rpc_url = %redacted_url,
                            attempt = attempt,
                            http_status = status.as_u16(),
                            backoff_ms = backoff.as_millis() as u64,
                            "rpc_retry_scheduled"
                        );
                        tokio::time::sleep(backoff).await;
                        continue;
                    }
                    if !status.is_success() {
                        return Err(SentinelError::RpcResponseError {
                            code: status.as_u16() as i64,
                            message: format!("HTTP {} error from RPC endpoint", status),
                        });
                    }

                    let mut body_bytes = Vec::new();
                    let mut response = response;
                    while let Some(chunk) =
                        response
                            .chunk()
                            .await
                            .map_err(|error| SentinelError::RpcResponseError {
                                code: -32603,
                                message: format!("Error reading response stream: {error}"),
                            })?
                    {
                        if body_bytes.len().saturating_add(chunk.len()) > MAX_RPC_RESPONSE_BYTES {
                            return Err(SentinelError::RpcResponseError {
                                code: -32600,
                                message: format!(
                                    "RPC response exceeded maximum allowed size limit of {} bytes",
                                    MAX_RPC_RESPONSE_BYTES
                                ),
                            });
                        }
                        body_bytes.extend_from_slice(&chunk);
                    }
                    return Ok(body_bytes);
                }
                Err(error) => {
                    info!(
                        rpc_url = %redacted_url,
                        attempt = attempt,
                        latency_ms = started_at.elapsed().as_millis() as u64,
                        error = %error,
                        "rpc_request_failed"
                    );

                    // Retry on connect errors or timeouts if attempts remain
                    if retryable
                        && (error.is_connect() || error.is_timeout())
                        && attempt < self.retry_policy.max_retries
                    {
                        let backoff = self.retry_policy.next_delay(attempt, None);
                        debug!(
                            rpc_url = %redacted_url,
                            attempt = attempt,
                            backoff_ms = backoff.as_millis() as u64,
                            "rpc_retry_scheduled"
                        );
                        tokio::time::sleep(backoff).await;
                        continue;
                    }
                    if error.is_timeout() {
                        return Err(SentinelError::RpcConnectionTimeout(format!(
                            "Timeout connecting to RPC at {}: {}",
                            redacted_url, error
                        )));
                    } else {
                        return Err(SentinelError::RpcConnectionTimeout(format!(
                            "Failed to connect to RPC at {}: {}",
                            redacted_url, error
                        )));
                    }
                }
            }
        }
    }

    /// Internal generic JSON-RPC 2.0 request dispatcher with retry and exponential backoff middleware.
    #[tracing::instrument(skip(self, params), fields(rpc.method = method, rpc.url = %self.rpc_url))]
    pub(super) async fn send_rpc_request<P, R>(
        &self,
        method: &'static str,
        params: P,
    ) -> SentinelResult<R>
    where
        P: Serialize + Send + Sync,
        R: for<'de> Deserialize<'de>,
    {
        let request_payload = JsonRpcRequest {
            jsonrpc: "2.0",
            id: 1,
            method,
            params,
        };
        let body = self
            .send_rpc_payload(&request_payload, method != "sendTransaction")
            .await?;
        let rpc_response: JsonRpcResponse<R> = serde_json::from_slice(&body).map_err(|error| {
            SentinelError::SerializationError(format!("Failed to parse JSON-RPC response: {error}"))
        })?;

        if let Some(error) = rpc_response.error {
            return Err(SentinelError::RpcResponseError {
                code: error.code,
                message: error.message,
            });
        }
        rpc_response
            .result
            .ok_or_else(|| SentinelError::RpcResponseError {
                code: -32603,
                message: "Missing result field in JSON-RPC 2.0 response".to_string(),
            })
    }
}

#[async_trait]
impl LedgerReader for SorobanRpcClient {
    async fn get_latest_ledger(&self) -> SentinelResult<LatestLedgerInfo> {
        let result: RpcLatestLedgerResult = self
            .send_rpc_request("getLatestLedger", serde_json::json!({}))
            .await?;

        Ok(LatestLedgerInfo::new(
            result.sequence,
            result.protocol_version,
        ))
    }

    async fn get_ledger_entries(
        &self,
        keys_xdr_base64: &[String],
    ) -> SentinelResult<Vec<RawLedgerEntry>> {
        if keys_xdr_base64.is_empty() {
            return Ok(Vec::new());
        }

        let chunks: Vec<&[String]> = keys_xdr_base64.chunks(MAX_KEYS_PER_REQUEST).collect();
        if chunks.len() == 1 {
            return get_ledger_entries_chunk(self, chunks[0]).await;
        }

        let mut all_entries = Vec::new();
        for (batch_index, batch) in chunks.chunks(MAX_BATCH_REQUESTS).enumerate() {
            let first_id = batch_index * MAX_BATCH_REQUESTS + 1;
            let requests: Vec<_> = batch
                .iter()
                .enumerate()
                .map(|(index, keys)| JsonRpcRequest {
                    jsonrpc: "2.0",
                    id: (first_id + index) as u64,
                    method: "getLedgerEntries",
                    params: GetLedgerEntriesParams { keys },
                })
                .collect();
            let mut responses = send_rpc_batch(self, &requests).await?;

            for id in first_id..first_id + batch.len() {
                let Some(response) = responses.remove(&(id as u64)) else {
                    tracing::warn!(rpc.request_id = id, "missing JSON-RPC batch response");
                    continue;
                };
                if let Some(error) = response.error {
                    tracing::warn!(
                        rpc.request_id = id,
                        rpc.error_code = error.code,
                        rpc.error_message = %error.message,
                        "getLedgerEntries batch item failed"
                    );
                    continue;
                }
                let Some(result) = response.result else {
                    tracing::warn!(
                        rpc.request_id = id,
                        "getLedgerEntries batch item omitted result"
                    );
                    continue;
                };
                let result: GetLedgerEntriesResult = match serde_json::from_value(result) {
                    Ok(result) => result,
                    Err(error) => {
                        tracing::warn!(
                            rpc.request_id = id,
                            error = %error,
                            "invalid getLedgerEntries batch item result"
                        );
                        continue;
                    }
                };
                if let Some(entries) = result.entries {
                    all_entries.extend(entries.into_iter().map(raw_ledger_entry));
                }
            }
        }

        Ok(all_entries)
    }

    async fn get_ledger_close_velocity(&self, latest_ledger: u32) -> SentinelResult<Option<f64>> {
        let samples = self
            .get_ledger_close_samples(latest_ledger, DEFAULT_SAMPLE_WINDOW_LEDGERS)
            .await?;
        Ok(samples.and_then(|samples| {
            let burn_rate = calculate_burn_rate(&samples, TARGET_LEDGER_CLOSE_SECONDS);
            burn_rate
                .is_measured()
                .then_some(burn_rate.seconds_per_ledger)
        }))
    }

    async fn get_ledger_close_samples(
        &self,
        latest_ledger: u32,
        sample_window: u32,
    ) -> SentinelResult<Option<Vec<LedgerCloseSample>>> {
        if latest_ledger < 2 || sample_window < 2 {
            return Ok(None);
        }

        let sample_limit = sample_window.min(latest_ledger);
        let start_ledger = latest_ledger.saturating_sub(sample_limit - 1).max(1);

        let params = GetLedgersParams {
            start_ledger,
            pagination: PaginationLimit {
                limit: sample_limit,
            },
        };

        let result: Result<GetLedgersResult, _> = self.send_rpc_request("getLedgers", params).await;
        let ledgers = match result {
            Ok(res) => res.ledgers.unwrap_or_default(),
            Err(_) => return Ok(None), // -32601 Method not found, or any transport error falls back
        };

        let samples: Vec<LedgerCloseSample> = ledgers
            .into_iter()
            .filter_map(|ledger| {
                ledger
                    .ledger_close_time
                    .filter(|close_time| *close_time > 0)
                    .map(|close_time| LedgerCloseSample::new(ledger.sequence, close_time))
            })
            .collect();

        if samples.len() < 2 {
            Ok(None)
        } else {
            Ok(Some(samples))
        }
    }
}

async fn get_ledger_entries_chunk(
    client: &SorobanRpcClient,
    keys: &[String],
) -> SentinelResult<Vec<RawLedgerEntry>> {
    let result: GetLedgerEntriesResult = client
        .send_rpc_request("getLedgerEntries", GetLedgerEntriesParams { keys })
        .await?;
    Ok(result
        .entries
        .unwrap_or_default()
        .into_iter()
        .map(raw_ledger_entry)
        .collect())
}

async fn send_rpc_batch(
    client: &SorobanRpcClient,
    requests: &[JsonRpcRequest<GetLedgerEntriesParams<'_>>],
) -> SentinelResult<BTreeMap<u64, JsonRpcResponse<serde_json::Value>>> {
    let body = client.send_rpc_payload(requests, true).await?;
    let batch: Vec<JsonRpcResponse<serde_json::Value>> =
        serde_json::from_slice(&body).map_err(|error| {
            SentinelError::SerializationError(format!(
                "Failed to parse JSON-RPC batch response: {error}"
            ))
        })?;
    let expected_ids: std::collections::HashSet<u64> =
        requests.iter().map(|request| request.id).collect();
    let mut responses = BTreeMap::new();
    for response in batch {
        let id = response
            .id
            .as_ref()
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| SentinelError::RpcResponseError {
                code: -32600,
                message: "JSON-RPC batch response has an invalid ID".to_string(),
            })?;
        if !expected_ids.contains(&id) || responses.insert(id, response).is_some() {
            return Err(SentinelError::RpcResponseError {
                code: -32600,
                message: format!(
                    "JSON-RPC batch response contains an unexpected or duplicate ID {id}"
                ),
            });
        }
    }
    Ok(responses)
}

fn raw_ledger_entry(item: RpcLedgerEntryItem) -> RawLedgerEntry {
    RawLedgerEntry::new(
        item.key,
        item.xdr,
        item.live_until_ledger_seq.unwrap_or(0),
        item.last_modified_ledger_seq.unwrap_or(0),
    )
}

#[async_trait]
impl EventStreamReader for SorobanRpcClient {
    async fn get_contract_events(
        &self,
        contract_id: &str,
        start_ledger: u32,
    ) -> SentinelResult<Vec<ContractEvent>> {
        let contract_bytes = parse_contract_id(contract_id)?;
        let canonical_id = stellar_strkey::Contract(contract_bytes).to_string();

        let mut cursor: Option<String> = None;
        let mut parsed = Vec::new();
        let mut seen = HashSet::new();

        for _ in 0..MAX_EVENT_PAGES {
            let params = GetEventsParams {
                start_ledger,
                filters: vec![GetEventsFilter {
                    contract_ids: vec![canonical_id.as_str()],
                }],
                pagination: GetEventsPagination {
                    limit: MAX_EVENTS_PER_PAGE,
                    cursor: cursor.as_deref(),
                },
            };

            let result: GetEventsResult = self.send_rpc_request("getEvents", params).await?;
            let page_len = result.events.len();
            let mut last_paging_token = None;

            for item in result.events {
                if let Some(token) = item.paging_token.clone() {
                    last_paging_token = Some(token);
                }
                if seen.insert(item.id.clone()) {
                    parsed.push(decode_contract_event(item));
                }
            }

            if page_len == 0 {
                break;
            }

            let next_cursor = result.cursor.or(last_paging_token);
            match next_cursor {
                Some(next) if cursor.as_deref() != Some(next.as_str()) => {
                    cursor = Some(next);
                }
                _ => break,
            }
        }

        Ok(parsed)
    }
}

fn decode_contract_event(item: RpcContractEventItem) -> ContractEvent {
    let topics = item
        .topic
        .iter()
        .filter_map(|topic| decode_scval(topic))
        .collect();
    let value = item.value.as_deref().and_then(decode_scval);

    ContractEvent::from_decoded(DecodedContractEvent {
        id: item.id,
        paging_token: item.paging_token,
        ledger: item.ledger,
        ledger_close_time: item.ledger_close_time,
        event_type: SorobanEventType::from_rpc_str(&item.event_type),
        contract_id: item.contract_id,
        topics,
        value,
        tx_hash: item.tx_hash,
        in_successful_contract_call: item.in_successful_contract_call.unwrap_or(false),
    })
}

fn decode_scval(base64: &str) -> Option<ScVal> {
    ScVal::from_xdr_base64(base64, safe_xdr_limits()).ok()
}

#[async_trait]
impl SimulationEngine for SorobanRpcClient {
    async fn simulate_transaction(
        &self,
        transaction_xdr_base64: &str,
        latest_ledger: u32,
    ) -> SentinelResult<crate::ports::RentEstimate> {
        let result =
            SorobanRpcClient::simulate_transaction(self, transaction_xdr_base64, "", latest_ledger)
                .await?;
        Ok(crate::ports::RentEstimate {
            min_resource_fee_stroops: result.min_resource_fee,
            latest_ledger: result.latest_ledger,
            cpu_instructions: result.cpu_instructions,
            memory_bytes: result.memory_bytes,
        })
    }
}

// --- Wire format DTOs ---

#[derive(Debug, Serialize)]
struct JsonRpcRequest<T> {
    jsonrpc: &'static str,
    id: u64,
    method: &'static str,
    params: T,
}

#[derive(Debug, Deserialize)]
struct JsonRpcResponse<T> {
    #[allow(dead_code)]
    jsonrpc: Option<String>,
    #[allow(dead_code)]
    id: Option<serde_json::Value>,
    result: Option<T>,
    error: Option<JsonRpcError>,
}

#[derive(Debug, Deserialize)]
struct JsonRpcError {
    code: i64,
    message: String,
    #[allow(dead_code)]
    data: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RpcLatestLedgerResult {
    sequence: u32,
    protocol_version: u32,
}

#[derive(Debug, Serialize)]
struct GetLedgerEntriesParams<'a> {
    keys: &'a [String],
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GetLedgerEntriesResult {
    #[serde(default)]
    entries: Option<Vec<RpcLedgerEntryItem>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RpcLedgerEntryItem {
    key: String,
    #[serde(alias = "entry")]
    xdr: String,
    #[serde(default)]
    live_until_ledger_seq: Option<u32>,
    #[serde(default)]
    last_modified_ledger_seq: Option<u32>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GetLedgersParams {
    start_ledger: u32,
    pagination: PaginationLimit,
}

#[derive(Debug, Serialize)]
struct PaginationLimit {
    limit: u32,
}

#[derive(Debug, Deserialize)]
struct GetLedgersResult {
    #[serde(default)]
    ledgers: Option<Vec<RpcLedgerItem>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RpcLedgerItem {
    sequence: u32,
    #[serde(deserialize_with = "deserialize_close_time_opt")]
    ledger_close_time: Option<i64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GetEventsParams<'a> {
    start_ledger: u32,
    filters: Vec<GetEventsFilter<'a>>,
    pagination: GetEventsPagination<'a>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GetEventsFilter<'a> {
    contract_ids: Vec<&'a str>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GetEventsPagination<'a> {
    limit: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    cursor: Option<&'a str>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GetEventsResult {
    #[serde(default)]
    events: Vec<RpcContractEventItem>,
    #[serde(default)]
    cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RpcContractEventItem {
    #[serde(rename = "type")]
    event_type: String,
    ledger: u32,
    #[serde(default, deserialize_with = "deserialize_close_time_opt")]
    ledger_close_time: Option<i64>,
    #[serde(default)]
    contract_id: Option<String>,
    #[serde(default)]
    id: String,
    #[serde(default)]
    paging_token: Option<String>,
    #[serde(default)]
    topic: Vec<String>,
    #[serde(default)]
    value: Option<String>,
    #[serde(default)]
    tx_hash: Option<String>,
    #[serde(default)]
    in_successful_contract_call: Option<bool>,
}

fn deserialize_close_time_opt<'de, D>(deserializer: D) -> Result<Option<i64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::Deserialize;
    let v = Option::<serde_json::Value>::deserialize(deserializer)?;
    match v {
        Some(serde_json::Value::String(s)) => {
            if let Ok(n) = s.parse::<i64>() {
                Ok(Some(n))
            } else {
                Ok(None)
            }
        }
        Some(serde_json::Value::Number(num)) => Ok(num.as_i64()),
        _ => Ok(None),
    }
}

fn parse_fee_percentiles(result: &serde_json::Value) -> SentinelResult<FeePercentiles> {
    let stats = result
        .get("sorobanInclusionFee")
        .or_else(|| result.get("inclusionFee"))
        .ok_or_else(|| {
            SentinelError::SerializationError(
                "getFeeStats response omitted inclusion fee percentiles".to_string(),
            )
        })?;
    let parse = |name: &str| -> SentinelResult<u64> {
        let value = stats.get(name).ok_or_else(|| {
            SentinelError::SerializationError(format!(
                "getFeeStats response omitted {name} percentile"
            ))
        })?;
        value
            .as_u64()
            .or_else(|| value.as_str().and_then(|value| value.parse::<u64>().ok()))
            .ok_or_else(|| {
                SentinelError::SerializationError(format!(
                    "getFeeStats returned an invalid {name} percentile"
                ))
            })
    };

    Ok(FeePercentiles {
        p50: parse("p50")?,
        p80: parse("p80")?,
        p90: parse("p90")?,
    })
}

/// Fuzzing entry point: parses a `getLatestLedger` JSON-RPC response body.
///
/// Mirrors the deserialization performed by [`SorobanRpcClient::get_latest_ledger`]
/// after the response size cap has been enforced by the transport.
#[cfg(feature = "fuzzing")]
#[doc(hidden)]
pub fn fuzz_parse_get_latest_ledger_response(body: &[u8]) -> SentinelResult<u32> {
    if body.len() > MAX_RPC_RESPONSE_BYTES {
        return Err(SentinelError::RpcResponseError {
            code: -32600,
            message: "RPC response exceeded maximum allowed size".to_string(),
        });
    }
    let response: JsonRpcResponse<RpcLatestLedgerResult> =
        serde_json::from_slice(body).map_err(|e| {
            SentinelError::SerializationError(format!("Failed to parse JSON-RPC response: {}", e))
        })?;
    if let Some(error) = response.error {
        return Err(SentinelError::RpcResponseError {
            code: error.code,
            message: error.message,
        });
    }
    response
        .result
        .map(|r| r.sequence)
        .ok_or_else(|| SentinelError::RpcResponseError {
            code: -32603,
            message: "Missing result field in JSON-RPC 2.0 response".to_string(),
        })
}

/// Fuzzing entry point: parses a `getLedgerEntries` JSON-RPC response body.
#[cfg(feature = "fuzzing")]
#[doc(hidden)]
pub fn fuzz_parse_get_ledger_entries_response(body: &[u8]) -> SentinelResult<usize> {
    if body.len() > MAX_RPC_RESPONSE_BYTES {
        return Err(SentinelError::RpcResponseError {
            code: -32600,
            message: "RPC response exceeded maximum allowed size".to_string(),
        });
    }
    let response: JsonRpcResponse<GetLedgerEntriesResult> =
        serde_json::from_slice(body).map_err(|e| {
            SentinelError::SerializationError(format!("Failed to parse JSON-RPC response: {}", e))
        })?;
    if let Some(error) = response.error {
        return Err(SentinelError::RpcResponseError {
            code: error.code,
            message: error.message,
        });
    }
    Ok(response
        .result
        .and_then(|r| r.entries)
        .map(|entries| entries.len())
        .unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn test_redact_rpc_url_userinfo() {
        let url = "http://user:pass@localhost:8000/soroban/rpc";
        let redacted = redact_rpc_url(url);
        assert!(!redacted.contains("user:pass"));
        assert!(redacted.contains("localhost"));
    }

    #[test]
    fn test_redact_rpc_url_api_key_query() {
        let url = "https://rpc.example.com?api_key=sk-12345&other=val";
        let redacted = redact_rpc_url(url);
        assert!(redacted.contains("redacted"));
        assert!(!redacted.contains("sk-12345"));
    }

    #[test]
    fn test_redact_rpc_url_soroban_address() {
        let url = "https://rpc.example.com/keys/GABC1234567890abcdefghijklmnopqrstuv";
        let redacted = redact_rpc_url(url);
        assert!(redacted.contains("redacted"));
        assert!(!redacted.contains("GABC1234567890abcdefghijklmnopqrstuv"));
    }

    #[test]
    fn test_redact_rpc_url_no_secrets() {
        let url = "https://soroban-testnet.stellar.org";
        let redacted = redact_rpc_url(url);
        assert_eq!(redacted, "https://soroban-testnet.stellar.org");
    }

    #[test]
    fn test_parse_latest_ledger_json() {
        let json = r#"{
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "id": "abc",
                "protocolVersion": 22,
                "sequence": 5678900
            }
        }"#;

        let res: JsonRpcResponse<RpcLatestLedgerResult> = serde_json::from_str(json).unwrap();
        let result = res.result.unwrap();
        assert_eq!(result.sequence, 5678900);
        assert_eq!(result.protocol_version, 22);
    }

    #[test]
    fn test_parse_ledger_entries_json() {
        let json = r#"{
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "latestLedger": 5678900,
                "entries": [
                    {
                        "key": "KEY_1_BASE64",
                        "xdr": "XDR_1_BASE64",
                        "liveUntilLedgerSeq": 5700000,
                        "lastModifiedLedgerSeq": 5650000
                    },
                    {
                        "key": "KEY_2_BASE64",
                        "entry": "XDR_2_BASE64"
                    }
                ]
            }
        }"#;

        let res: JsonRpcResponse<GetLedgerEntriesResult> = serde_json::from_str(json).unwrap();
        let entries = res.result.unwrap().entries.unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].key, "KEY_1_BASE64");
        assert_eq!(entries[0].xdr, "XDR_1_BASE64");
        assert_eq!(entries[0].live_until_ledger_seq, Some(5700000));
        assert_eq!(entries[1].xdr, "XDR_2_BASE64");
        assert_eq!(entries[1].live_until_ledger_seq, None);
    }

    #[test]
    fn test_parse_jsonrpc_error() {
        let json = r#"{
            "jsonrpc": "2.0",
            "id": 1,
            "error": {
                "code": -32600,
                "message": "Invalid request"
            }
        }"#;

        let res: JsonRpcResponse<RpcLatestLedgerResult> = serde_json::from_str(json).unwrap();
        assert!(res.result.is_none());
        assert!(res.error.is_some());
        let err = res.error.unwrap();
        assert_eq!(err.code, -32600);
        assert_eq!(err.message, "Invalid request");
    }

    #[tokio::test]
    async fn test_wiremock_get_latest_ledger_success() {
        let mock_server = MockServer::start().await;

        let response_body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "protocolVersion": 21,
                "sequence": 5400000
            }
        });

        Mock::given(method("POST"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(response_body))
            .mount(&mock_server)
            .await;

        let client = SorobanRpcClient::new(mock_server.uri());
        let info = client.get_latest_ledger().await.unwrap();

        assert_eq!(info.sequence, 5400000);
        assert_eq!(info.protocol_version, 21);
    }

    #[tokio::test]
    async fn test_wiremock_get_ledger_entries_success() {
        let mock_server = MockServer::start().await;

        let response_body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "latestLedger": 5400000,
                "entries": [
                    {
                        "key": "TEST_KEY_BASE64",
                        "xdr": "TEST_XDR_BASE64",
                        "liveUntilLedgerSeq": 5500000,
                        "lastModifiedLedgerSeq": 5350000
                    }
                ]
            }
        });

        Mock::given(method("POST"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(response_body))
            .mount(&mock_server)
            .await;

        let client = SorobanRpcClient::new(mock_server.uri());
        let entries = client
            .get_ledger_entries(&["TEST_KEY_BASE64".to_string()])
            .await
            .unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].key_xdr_base64, "TEST_KEY_BASE64");
        assert_eq!(entries[0].entry_xdr_base64, "TEST_XDR_BASE64");
        assert_eq!(entries[0].live_until_ledger_seq, 5500000);
        assert_eq!(entries[0].last_modified_ledger_seq, 5350000);
    }

    #[tokio::test]
    async fn test_wiremock_rpc_error_handling() {
        let mock_server = MockServer::start().await;

        let response_body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "error": {
                "code": -32601,
                "message": "Method not found"
            }
        });

        Mock::given(method("POST"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(response_body))
            .mount(&mock_server)
            .await;

        let client = SorobanRpcClient::new(mock_server.uri());
        let res = client.get_latest_ledger().await;

        assert!(matches!(
            res,
            Err(SentinelError::RpcResponseError { code: -32601, .. })
        ));
    }

    #[cfg(feature = "fuzzing")]
    #[test]
    fn test_fuzz_parse_entry_points_reject_oversized_and_accept_valid() {
        let latest = br#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":22,"sequence":42}}"#;
        assert_eq!(fuzz_parse_get_latest_ledger_response(latest).unwrap(), 42);
        let entries = br#"{"jsonrpc":"2.0","id":1,"result":{"entries":[]}}"#;
        assert_eq!(fuzz_parse_get_ledger_entries_response(entries).unwrap(), 0);
        let huge = vec![b' '; MAX_RPC_RESPONSE_BYTES + 1];
        assert!(fuzz_parse_get_latest_ledger_response(&huge).is_err());
        assert!(fuzz_parse_get_ledger_entries_response(&huge).is_err());
    }

    #[test]
    fn test_validate_rpc_url_schemes() {
        // Valid http and https
        assert!(validate_rpc_url("https://soroban-testnet.stellar.org").is_ok());
        assert!(validate_rpc_url("http://localhost:8000/soroban/rpc").is_ok());

        // Invalid schemes rejected
        let err = validate_rpc_url("file:///etc/passwd").unwrap_err();
        assert!(matches!(err, SentinelError::InvalidRpcUrl(_)));

        let err = validate_rpc_url("ftp://ftp.example.com").unwrap_err();
        assert!(matches!(err, SentinelError::InvalidRpcUrl(_)));

        // Malformed URL rejected
        let err = validate_rpc_url("not_a_valid_url").unwrap_err();
        assert!(matches!(err, SentinelError::InvalidRpcUrl(_)));
    }

    #[tokio::test]
    async fn test_wiremock_malformed_json_response() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_string("THIS IS NOT JSON"))
            .mount(&mock_server)
            .await;

        let client = SorobanRpcClient::new(mock_server.uri());
        let res = client.get_latest_ledger().await;

        assert!(matches!(res, Err(SentinelError::SerializationError(_))));
    }

    #[tokio::test]
    async fn test_wiremock_http_error_status() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(500).set_body_string("Internal Server Error"))
            .mount(&mock_server)
            .await;

        let client = SorobanRpcClient::new(mock_server.uri());
        let res = client.get_latest_ledger().await;

        assert!(matches!(
            res,
            Err(SentinelError::RpcResponseError { code: 500, .. })
        ));
    }

    #[test]
    fn test_retry_policy_next_delay_and_capping() {
        let policy = RetryPolicy {
            max_retries: 5,
            base_delay: Duration::from_millis(100),
            max_delay: Duration::from_millis(1000),
            jitter: Jitter::None,
            seed: None,
        };

        // attempt 1: 100ms * 2^0 = 100ms
        assert_eq!(policy.next_delay(1, None), Duration::from_millis(100));
        // attempt 2: 100ms * 2^1 = 200ms
        assert_eq!(policy.next_delay(2, None), Duration::from_millis(200));
        // attempt 3: 100ms * 2^2 = 400ms
        assert_eq!(policy.next_delay(3, None), Duration::from_millis(400));
        // attempt 4: 100ms * 2^3 = 800ms
        assert_eq!(policy.next_delay(4, None), Duration::from_millis(800));
        // attempt 5: 100ms * 2^4 = 1600ms -> capped to 1000ms
        assert_eq!(policy.next_delay(5, None), Duration::from_millis(1000));
        // attempt 10: capped to 1000ms
        assert_eq!(policy.next_delay(10, None), Duration::from_millis(1000));

        // Retry-After honoured
        assert_eq!(
            policy.next_delay(1, Some(Duration::from_millis(500))),
            Duration::from_millis(500)
        );

        // Retry-After clamped to max_delay
        assert_eq!(
            policy.next_delay(1, Some(Duration::from_millis(5000))),
            Duration::from_millis(1000)
        );
    }

    #[test]
    fn test_retry_policy_jitter_determinism_with_seed() {
        let policy = RetryPolicy {
            max_retries: 3,
            base_delay: Duration::from_millis(100),
            max_delay: Duration::from_millis(1000),
            jitter: Jitter::Full,
            seed: Some(42),
        };

        let delay1 = policy.next_delay(2, None);
        let delay2 = policy.next_delay(2, None);
        assert_eq!(delay1, delay2); // Seeded jitter is deterministic
        assert!(delay1 <= Duration::from_millis(200));
    }

    #[tokio::test]
    async fn test_wiremock_get_ledgers_velocity_success() {
        let mock_server = MockServer::start().await;

        let response_body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "ledgers": [
                    {
                        "sequence": 5399901,
                        "ledgerCloseTime": "1700000000"
                    },
                    {
                        "sequence": 5400000,
                        "ledgerCloseTime": "1700000500"
                    }
                ]
            }
        });

        Mock::given(method("POST"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(response_body))
            .mount(&mock_server)
            .await;

        let client = SorobanRpcClient::new(mock_server.uri());
        let velocity = client.get_ledger_close_velocity(5400000).await.unwrap();

        // (1700000500 - 1700000000) / (5400000 - 5399901) = 500 / 99 ~= 5.0505
        assert!(velocity.is_some());
        let v = velocity.unwrap();
        assert!((v - 5.0505).abs() < 1e-3);
    }

    #[tokio::test]
    async fn test_wiremock_get_ledgers_method_not_found_fallback() {
        let mock_server = MockServer::start().await;

        let response_body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "error": {
                "code": -32601,
                "message": "Method not found"
            }
        });

        Mock::given(method("POST"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(response_body))
            .mount(&mock_server)
            .await;

        let client = SorobanRpcClient::new(mock_server.uri());
        let velocity = client.get_ledger_close_velocity(5400000).await.unwrap();
        // Method not found returns Ok(None), so scan falls back to 5.0s
        assert_eq!(velocity, None);
    }

    #[tokio::test]
    async fn test_wiremock_get_ledgers_malformed_close_time_fallback() {
        let mock_server = MockServer::start().await;

        let response_body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "ledgers": [
                    {
                        "sequence": 5399901,
                        "ledgerCloseTime": "not-a-number"
                    },
                    {
                        "sequence": 5400000,
                        "ledgerCloseTime": "not-a-number-either"
                    }
                ]
            }
        });

        Mock::given(method("POST"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(response_body))
            .mount(&mock_server)
            .await;

        let client = SorobanRpcClient::new(mock_server.uri());
        let velocity = client.get_ledger_close_velocity(5400000).await.unwrap();
        assert_eq!(velocity, None);
    }
}
