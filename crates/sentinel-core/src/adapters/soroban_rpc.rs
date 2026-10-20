//! Soroban JSON-RPC 2.0 client adapter implementing [`LedgerReader`].

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::time::Duration;

use crate::error::{SentinelError, SentinelResult};
use crate::ports::reader::{LatestLedgerInfo, LedgerReader, RawLedgerEntry};

/// Maximum number of keys allowed in a single `getLedgerEntries` request by Soroban RPC.
pub const MAX_KEYS_PER_REQUEST: usize = 200;

/// Default maximum retry attempts for transient errors (HTTP 429 / 503).
pub const DEFAULT_MAX_RETRIES: u32 = 3;

/// Default initial retry delay in milliseconds.
pub const DEFAULT_BASE_RETRY_DELAY_MS: u64 = 100;

/// Retry configuration for handling transient HTTP and network errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    /// Maximum number of attempts before propagating the error.
    pub max_retries: u32,
    /// Base delay applied to exponential backoff calculation.
    pub base_delay: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: DEFAULT_MAX_RETRIES,
            base_delay: Duration::from_millis(DEFAULT_BASE_RETRY_DELAY_MS),
        }
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
    /// Create a new `SorobanRpcClient` with default timeout (10 seconds) and standard retry policy.
    pub fn new(rpc_url: impl Into<String>) -> Self {
        let http_client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .expect("Failed to initialize reqwest HTTP client");

        Self {
            rpc_url: rpc_url.into(),
            http_client,
            retry_policy: RetryPolicy::default(),
        }
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
                        let backoff =
                            self.retry_policy.base_delay * 2u32.saturating_pow(attempt - 1);
                        tokio::time::sleep(backoff).await;
                        continue;
                    }

                    if !status.is_success() {
                        return Err(SentinelError::RpcResponseError {
                            code: status.as_u16() as i64,
                            message: format!("HTTP {} error from RPC endpoint", status),
                        });
                    }

                    let rpc_response: JsonRpcResponse<R> = response.json().await.map_err(|e| {
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
                        let backoff =
                            self.retry_policy.base_delay * 2u32.saturating_pow(attempt - 1);
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
}
