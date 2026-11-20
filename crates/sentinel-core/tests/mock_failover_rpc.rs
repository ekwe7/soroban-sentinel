use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use sentinel_core::adapters::{
    FailoverConfig, MultiRpcFailoverReader, RetryPolicy, SorobanRpcClient,
};
use sentinel_core::error::SentinelError;
use sentinel_core::ports::{LedgerReader, MockLedgerReader, ReaderHealth};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn rpc_client(url: String) -> SorobanRpcClient {
    SorobanRpcClient::new(url).with_retry_policy(RetryPolicy {
        max_retries: 1,
        base_delay: Duration::ZERO,
        ..RetryPolicy::default()
    })
}

fn latest_ledger_response(sequence: u32) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "result": {
            "sequence": sequence,
            "protocolVersion": 22
        }
    }))
}

#[tokio::test]
async fn fails_over_from_primary_503_to_secondary() {
    let primary = MockServer::start().await;
    let secondary = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(503))
        .expect(1)
        .mount(&primary)
        .await;

    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(latest_ledger_response(8_765_432))
        .expect(1)
        .mount(&secondary)
        .await;

    let reader = MultiRpcFailoverReader::with_config(
        vec![rpc_client(primary.uri()), rpc_client(secondary.uri())],
        FailoverConfig {
            probe_interval: Duration::from_secs(60),
            ..FailoverConfig::default()
        },
    );

    let latest = reader
        .get_latest_ledger()
        .await
        .expect("secondary should serve the request");
    assert_eq!(latest.sequence, 8_765_432);

    let health = reader.health();
    assert_eq!(health[0].health, ReaderHealth::Unhealthy);
    assert_eq!(health[0].consecutive_errors, 1);
    assert_eq!(health[1].health, ReaderHealth::Healthy);
    primary.verify().await;
    secondary.verify().await;
}

#[tokio::test]
async fn recovers_primary_after_successful_health_probe() {
    let primary = MockServer::start().await;
    let secondary = MockServer::start().await;

    let primary_available = Arc::new(AtomicBool::new(false));
    let primary_available_for_response = Arc::clone(&primary_available);
    let primary_requests = Arc::new(AtomicU32::new(0));
    let primary_requests_for_response = Arc::clone(&primary_requests);

    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(move |_: &wiremock::Request| {
            primary_requests_for_response.fetch_add(1, Ordering::SeqCst);
            if primary_available_for_response.load(Ordering::SeqCst) {
                latest_ledger_response(9_000_001)
            } else {
                ResponseTemplate::new(503)
            }
        })
        .mount(&primary)
        .await;

    let secondary_requests = Arc::new(AtomicU32::new(0));
    let secondary_requests_for_response = Arc::clone(&secondary_requests);
    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(move |_: &wiremock::Request| {
            secondary_requests_for_response.fetch_add(1, Ordering::SeqCst);
            latest_ledger_response(9_000_000)
        })
        .mount(&secondary)
        .await;

    let reader = MultiRpcFailoverReader::with_config(
        vec![rpc_client(primary.uri()), rpc_client(secondary.uri())],
        FailoverConfig {
            probe_interval: Duration::ZERO,
            ..FailoverConfig::default()
        },
    );

    assert_eq!(
        reader
            .get_latest_ledger()
            .await
            .expect("secondary should serve while primary is down")
            .sequence,
        9_000_000
    );
    assert_eq!(reader.health()[0].health, ReaderHealth::Unhealthy);

    primary_available.store(true, Ordering::SeqCst);
    assert_eq!(
        reader
            .get_latest_ledger()
            .await
            .expect("recovered primary should serve after its health probe")
            .sequence,
        9_000_001
    );

    assert_eq!(primary_requests.load(Ordering::SeqCst), 3);
    assert_eq!(secondary_requests.load(Ordering::SeqCst), 1);
    assert_eq!(reader.health()[0].health, ReaderHealth::Healthy);
}

#[tokio::test]
async fn does_not_fail_over_for_client_validation_errors() {
    let secondary = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(latest_ledger_response(9_100_000))
        .expect(0)
        .mount(&secondary)
        .await;

    let primary: Arc<dyn LedgerReader> = Arc::new(MockLedgerReader::new().with_error(
        SentinelError::InvalidXdrKey("invalid client key".to_string()),
    ));
    let reader = MultiRpcFailoverReader::from_readers(
        vec![
            ("primary".to_string(), primary),
            (
                "secondary".to_string(),
                Arc::new(rpc_client(secondary.uri())),
            ),
        ],
        FailoverConfig::default(),
    );

    let error = reader
        .get_latest_ledger()
        .await
        .expect_err("validation errors should be returned without trying backups");
    assert!(matches!(error, SentinelError::InvalidXdrKey(_)));
    assert_eq!(reader.health()[0].health, ReaderHealth::Healthy);
    secondary.verify().await;
}
