use std::time::Duration;

use serde_json::Value;
use stellar_xdr::curr::{
    ExtensionPoint, LedgerFootprint, LedgerKey, Limits, SorobanResources, SorobanTransactionData,
    WriteXdr,
};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use sentinel_core::adapters::{RetryPolicy, SorobanRpcClient};
use sentinel_core::domain::{footprint_cache_key, SimulationCache};

fn transaction_data_xdr() -> String {
    let data = SorobanTransactionData {
        ext: ExtensionPoint::V0,
        resources: SorobanResources {
            footprint: LedgerFootprint {
                read_only: Vec::<LedgerKey>::new().try_into().expect("empty read set"),
                read_write: Vec::<LedgerKey>::new().try_into().expect("empty write set"),
            },
            instructions: 4_200,
            read_bytes: 300,
            write_bytes: 120,
        },
        resource_fee: 1_234,
    };
    data.to_xdr_base64(Limits::none()).expect("valid XDR")
}

async fn counting_rpc_server() -> MockServer {
    let server = MockServer::start().await;
    let transaction_data = transaction_data_xdr();
    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(move |request: &wiremock::Request| {
            let body: Value = serde_json::from_slice(&request.body).expect("valid JSON-RPC body");
            assert_eq!(body["method"], "simulateTransaction");
            ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": {
                    "minResourceFee": "98765",
                    "latestLedger": 5_000,
                    "transactionData": transaction_data,
                }
            }))
        })
        .mount(&server)
        .await;
    server
}

fn client(server: &MockServer, with_cache: bool) -> SorobanRpcClient {
    let client = SorobanRpcClient::new(server.uri()).with_retry_policy(RetryPolicy {
        max_retries: 1,
        base_delay: Duration::from_millis(1),
        ..Default::default()
    });
    if with_cache {
        client.with_simulation_cache(SimulationCache::new(8, 100))
    } else {
        client
    }
}

#[test]
fn footprint_keys_are_deterministic_and_distinct() {
    let first = footprint_cache_key("footprint-A");
    assert_eq!(first, footprint_cache_key("footprint-A"));
    assert_ne!(first, footprint_cache_key("footprint-B"));
}

#[tokio::test]
async fn cache_hit_avoids_rpc_and_miss_populates_cache() {
    let server = counting_rpc_server().await;
    let client = client(&server, true);

    let first = client
        .simulate_transaction("tx-xdr", "footprint-A", 5_000)
        .await
        .expect("first simulation");
    let second = client
        .simulate_transaction("tx-xdr", "footprint-A", 5_000)
        .await
        .expect("cached simulation");

    assert_eq!(first, second);
    assert_eq!(first.min_resource_fee, 98_765);
    assert_eq!(first.latest_ledger, 5_000);
    assert_eq!(first.cpu_instructions, 4_200);
    assert_eq!(first.memory_bytes, 420);
    assert_eq!(server.received_requests().await.expect("requests").len(), 1);

    client
        .simulate_transaction("tx-xdr", "footprint-B", 5_000)
        .await
        .expect("second footprint simulation");
    assert_eq!(server.received_requests().await.expect("requests").len(), 2);
}

#[tokio::test]
async fn ledger_age_boundary_controls_cache_validity() {
    let server = counting_rpc_server().await;
    let client = client(&server, true);

    client
        .simulate_transaction("tx-xdr", "footprint-A", 5_000)
        .await
        .expect("initial simulation");
    client
        .simulate_transaction("tx-xdr", "footprint-A", 5_100)
        .await
        .expect("entry at exact max ledger age");
    assert_eq!(server.received_requests().await.expect("requests").len(), 1);

    client
        .simulate_transaction("tx-xdr", "footprint-A", 5_101)
        .await
        .expect("stale entry");
    assert_eq!(server.received_requests().await.expect("requests").len(), 2);
}

#[tokio::test]
async fn disabled_cache_always_simulates() {
    let server = counting_rpc_server().await;
    let client = client(&server, false);
    assert!(client.simulation_cache().is_none());

    for _ in 0..2 {
        client
            .simulate_transaction("tx-xdr", "footprint-A", 5_000)
            .await
            .expect("simulation without cache");
    }
    assert_eq!(server.received_requests().await.expect("requests").len(), 2);
}

#[tokio::test]
async fn concurrent_simulations_with_distinct_footprints_stay_bounded() {
    let server = counting_rpc_server().await;
    let client = SorobanRpcClient::new(server.uri())
        .with_retry_policy(RetryPolicy {
            max_retries: 1,
            base_delay: Duration::from_millis(1),
            ..Default::default()
        })
        .with_simulation_cache(SimulationCache::new(4, 100));

    let mut handles = Vec::new();
    for index in 0..16 {
        let client = client.clone();
        handles.push(tokio::spawn(async move {
            client
                .simulate_transaction("tx-xdr", &format!("footprint-{index}"), 5_000)
                .await
                .expect("concurrent simulation")
        }));
    }
    for handle in handles {
        handle.await.expect("simulation task");
    }

    assert!(client.simulation_cache().expect("cache").len() <= 4);
}
