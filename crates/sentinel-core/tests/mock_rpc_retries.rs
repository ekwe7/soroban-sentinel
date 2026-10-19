use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use sentinel_core::adapters::{RetryPolicy, SorobanRpcClient};
use sentinel_core::error::SentinelError;
use sentinel_core::ports::LedgerReader;

#[tokio::test]
async fn test_retry_recovery_after_two_http_503_failures() {
    let mock_server = MockServer::start().await;

    let call_count = Arc::new(AtomicU32::new(0));
    let call_count_clone = Arc::clone(&call_count);

    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(move |_: &wiremock::Request| {
            let attempt = call_count_clone.fetch_add(1, Ordering::SeqCst) + 1;
            if attempt < 3 {
                // Fail attempts 1 and 2 with HTTP 503 Service Unavailable
                ResponseTemplate::new(503)
            } else {
                // Succeed on attempt 3 with HTTP 200
                let body = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "result": {
                        "sequence": 5550000,
                        "protocolVersion": 22
                    }
                });
                ResponseTemplate::new(200).set_body_json(body)
            }
        })
        .expect(3) // Exactly 3 attempts expected
        .mount(&mock_server)
        .await;

    let client = SorobanRpcClient::new(mock_server.uri()).with_retry_policy(RetryPolicy {
        max_retries: 3,
        base_delay: Duration::from_millis(5),
    });

    let ledger_info = client
        .get_latest_ledger()
        .await
        .expect("Should succeed on 3rd attempt after two 503 errors");

    assert_eq!(ledger_info.sequence, 5550000);
    assert_eq!(ledger_info.protocol_version, 22);
    assert_eq!(call_count.load(Ordering::SeqCst), 3);

    mock_server.verify().await;
}

#[tokio::test]
async fn test_retry_recovery_after_http_429_rate_limiting() {
    let mock_server = MockServer::start().await;

    let call_count = Arc::new(AtomicU32::new(0));
    let call_count_clone = Arc::clone(&call_count);

    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(move |_: &wiremock::Request| {
            let attempt = call_count_clone.fetch_add(1, Ordering::SeqCst) + 1;
            if attempt == 1 {
                // Fail attempt 1 with HTTP 429 Too Many Requests
                ResponseTemplate::new(429)
            } else {
                // Succeed on attempt 2
                let body = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "result": {
                        "sequence": 6000000,
                        "protocolVersion": 22
                    }
                });
                ResponseTemplate::new(200).set_body_json(body)
            }
        })
        .expect(2)
        .mount(&mock_server)
        .await;

    let client = SorobanRpcClient::new(mock_server.uri()).with_retry_policy(RetryPolicy {
        max_retries: 3,
        base_delay: Duration::from_millis(5),
    });

    let ledger_info = client
        .get_latest_ledger()
        .await
        .expect("Should succeed on 2nd attempt after 429 rate limit");

    assert_eq!(ledger_info.sequence, 6000000);
    assert_eq!(call_count.load(Ordering::SeqCst), 2);

    mock_server.verify().await;
}

#[tokio::test]
async fn test_retry_exhaustion_propagates_error() {
    let mock_server = MockServer::start().await;

    // Constantly returns HTTP 503
    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(503))
        .expect(3)
        .mount(&mock_server)
        .await;

    let client = SorobanRpcClient::new(mock_server.uri()).with_retry_policy(RetryPolicy {
        max_retries: 3,
        base_delay: Duration::from_millis(5),
    });

    let result = client.get_latest_ledger().await;

    assert!(
        matches!(
            result,
            Err(SentinelError::RpcResponseError { code: 503, .. })
        ),
        "Expected RpcResponseError with code 503, got: {:?}",
        result
    );

    mock_server.verify().await;
}
