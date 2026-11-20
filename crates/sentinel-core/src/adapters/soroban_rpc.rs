//! Soroban JSON-RPC 2.0 client adapter implementing [`LedgerReader`].

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::time::Duration;
use stellar_xdr::curr::{ReadXdr, ScVal};

use crate::adapters::xdr_keys::{parse_contract_id, safe_xdr_limits};
use crate::domain::events::{ContractEvent, DecodedContractEvent, SorobanEventType};
use crate::error::{SentinelError, SentinelResult};
use crate::ports::events::EventStreamReader;
use crate::ports::reader::{LatestLedgerInfo, LedgerReader, RawLedgerEntry};

/// Maximum number of keys allowed in a single `getLedgerEntries` request by Soroban RPC.
pub const MAX_KEYS_PER_REQUEST: usize = 200;

/// Maximum number of events requested per `getEvents` page.
pub const MAX_EVENTS_PER_PAGE: u32 = 100;

/// Safety bound on `getEvents` pagination loops to prevent unbounded reads.
pub const MAX_EVENT_PAGES: usize = 100;

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

/// JSON-RPC 2.0 client for interacting with Soroban RPC nodes.
#[derive(Debug, Clone)]
pub struct SorobanRpcClient {
    rpc_url: String,
    http_client: reqwest::Client,
    retry_policy: RetryPolicy,
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
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        Self {
            rpc_url: rpc_url.into(),
            http_client,
            retry_policy: RetryPolicy::default(),
        }
    }

    /// Set a custom request timeout.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.http_client = reqwest::Client::builder()
            .timeout(timeout)
            .redirect(reqwest::redirect::Policy::limited(3))
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

    /// Internal generic JSON-RPC 2.0 request dispatcher with retry and exponential backoff middleware.
    async fn send_rpc_request<P, R>(&self, method: &'static str, params: P) -> SentinelResult<R>
    where
        P: Serialize + Send,
        R: for<'de> Deserialize<'de>,
    {
        let request_payload = JsonRpcRequest {
            jsonrpc: "2.0",
            id: 1,
            method,
            params,
        };

        let mut attempt: u32 = 0;
        loop {
            attempt += 1;

            let response_result = self
                .http_client
                .post(&self.rpc_url)
                .json(&request_payload)
                .send()
                .await;

            match response_result {
                Ok(response) => {
                    let status = response.status();

                    // Check for rate limiting (429) or transient server unavailability (503)
                    if (status == reqwest::StatusCode::TOO_MANY_REQUESTS
                        || status == reqwest::StatusCode::SERVICE_UNAVAILABLE)
                        && attempt < self.retry_policy.max_retries
                    {
                        let retry_after = parse_retry_after(response.headers());
                        let backoff = self.retry_policy.next_delay(attempt, retry_after);
                        tokio::time::sleep(backoff).await;
                        continue;
                    }

                    if !status.is_success() {
                        return Err(SentinelError::RpcResponseError {
                            code: status.as_u16() as i64,
                            message: format!("HTTP {} error from RPC endpoint", status),
                        });
                    }

                    // Enforce streaming body size limit to prevent memory exhaustion
                    let mut body_bytes = Vec::new();
                    let mut response = response;
                    while let Some(chunk) =
                        response
                            .chunk()
                            .await
                            .map_err(|e| SentinelError::RpcResponseError {
                                code: -32603,
                                message: format!("Error reading response stream: {}", e),
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

                    let rpc_response: JsonRpcResponse<R> = serde_json::from_slice(&body_bytes)
                        .map_err(|e| {
                            SentinelError::SerializationError(format!(
                                "Failed to parse JSON-RPC response: {}",
                                e
                            ))
                        })?;

                    if let Some(error) = rpc_response.error {
                        return Err(SentinelError::RpcResponseError {
                            code: error.code,
                            message: error.message,
                        });
                    }

                    return rpc_response
                        .result
                        .ok_or_else(|| SentinelError::RpcResponseError {
                            code: -32603,
                            message: "Missing result field in JSON-RPC 2.0 response".to_string(),
                        });
                }
                Err(e) => {
                    // Retry on connect errors or timeouts if attempts remain
                    if (e.is_connect() || e.is_timeout()) && attempt < self.retry_policy.max_retries
                    {
                        let backoff = self.retry_policy.next_delay(attempt, None);
                        tokio::time::sleep(backoff).await;
                        continue;
                    }

                    if e.is_timeout() {
                        return Err(SentinelError::RpcConnectionTimeout(format!(
                            "Timeout connecting to RPC at {}: {}",
                            self.rpc_url, e
                        )));
                    } else {
                        return Err(SentinelError::RpcConnectionTimeout(format!(
                            "Failed to connect to RPC at {}: {}",
                            self.rpc_url, e
                        )));
                    }
                }
            }
        }
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

        let mut all_entries = Vec::new();

        // Enforce Soroban RPC request ceiling of <= 200 keys per query chunk
        for chunk in keys_xdr_base64.chunks(MAX_KEYS_PER_REQUEST) {
            let params = GetLedgerEntriesParams { keys: chunk };

            let result: GetLedgerEntriesResult =
                self.send_rpc_request("getLedgerEntries", params).await?;

            if let Some(entries) = result.entries {
                for item in entries {
                    all_entries.push(RawLedgerEntry::new(
                        item.key,
                        item.xdr,
                        item.live_until_ledger_seq.unwrap_or(0),
                        item.last_modified_ledger_seq.unwrap_or(0),
                    ));
                }
            }
        }

        Ok(all_entries)
    }

    async fn get_ledger_close_velocity(&self, latest_ledger: u32) -> SentinelResult<Option<f64>> {
        if latest_ledger < 2 {
            return Ok(None);
        }

        // Sample up to the last 100 ledgers
        let sample_limit = 100u32;
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

        if ledgers.len() < 2 {
            return Ok(None);
        }

        let first = &ledgers[0];
        let last = &ledgers[ledgers.len() - 1];

        let (Some(t_first), Some(t_last)) = (first.ledger_close_time, last.ledger_close_time)
        else {
            return Ok(None);
        };

        if last.sequence <= first.sequence || t_last <= t_first {
            return Ok(None);
        }

        let delta_seq = (last.sequence - first.sequence) as f64;
        let delta_time = (t_last - t_first) as f64;
        let velocity = delta_time / delta_seq;

        // Plausibility check: between 1.0s and 60.0s
        if (1.0..=60.0).contains(&velocity) {
            Ok(Some(velocity))
        } else {
            Ok(None)
        }
    }
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
