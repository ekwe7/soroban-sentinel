use serde_json::Value;
use std::time::Duration;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use sentinel_core::adapters::{RetryPolicy, SorobanRpcClient, MAX_KEYS_PER_REQUEST};
use sentinel_core::ports::LedgerReader;

#[tokio::test]
async fn test_rpc_chunking_with_250_keys() {
    let mock_server = MockServer::start().await;

    // Generate 250 unique test keys
    let keys: Vec<String> = (0..250).map(|i| format!("TEST_KEY_{i}")).collect();

    // Mock handler for getLedgerEntries
    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(|req: &wiremock::Request| {
            let body: Value = serde_json::from_slice(&req.body).expect("Valid JSON");
            assert_eq!(body["method"], "getLedgerEntries");

            let keys_chunk = body["params"]["keys"]
                .as_array()
                .expect("Keys array in params");

            let entries: Vec<Value> = keys_chunk
                .iter()
                .map(|k| {
                    serde_json::json!({
                        "key": k.as_str().unwrap(),
                        "xdr": format!("XDR_FOR_{}", k.as_str().unwrap()),
                        "liveUntilLedgerSeq": 6000000,
                        "lastModifiedLedgerSeq": 5900000,
                    })
                })
                .collect();

            let response_body = serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": {
                    "latestLedger": 5950000,
                    "entries": entries
                }
            });

            ResponseTemplate::new(200).set_body_json(response_body)
        })
        .expect(2) // Expect exactly 2 requests for 250 keys
        .mount(&mock_server)
        .await;

    let client = SorobanRpcClient::new(mock_server.uri()).with_retry_policy(RetryPolicy {
        max_retries: 1,
        base_delay: Duration::from_millis(1),
    });

    let results = client
        .get_ledger_entries(&keys)
        .await
        .expect("Chunked queries should succeed");

    // Verify all 250 entries were retrieved and assembled
    assert_eq!(results.len(), 250);
    for (i, entry) in results.iter().enumerate() {
        assert_eq!(entry.key_xdr_base64, format!("TEST_KEY_{i}"));
        assert_eq!(entry.entry_xdr_base64, format!("XDR_FOR_TEST_KEY_{i}"));
        assert_eq!(entry.live_until_ledger_seq, 6000000);
        assert_eq!(entry.last_modified_ledger_seq, 5900000);
    }

    // Verify the mock expectations were satisfied (exactly 2 requests)
    mock_server.verify().await;

    // Additionally inspect received requests to verify chunk sizes: 200 then 50
    let received_requests = mock_server.received_requests().await.unwrap();
    assert_eq!(received_requests.len(), 2);

    let body_1: Value = serde_json::from_slice(&received_requests[0].body).unwrap();
    let body_2: Value = serde_json::from_slice(&received_requests[1].body).unwrap();

    let keys_req_1 = body_1["params"]["keys"].as_array().unwrap();
    let keys_req_2 = body_2["params"]["keys"].as_array().unwrap();

    assert_eq!(keys_req_1.len(), MAX_KEYS_PER_REQUEST); // 200 keys
    assert_eq!(keys_req_2.len(), 50); // 50 keys
    assert_eq!(keys_req_1[0].as_str().unwrap(), "TEST_KEY_0");
    assert_eq!(keys_req_1[199].as_str().unwrap(), "TEST_KEY_199");
    assert_eq!(keys_req_2[0].as_str().unwrap(), "TEST_KEY_200");
    assert_eq!(keys_req_2[49].as_str().unwrap(), "TEST_KEY_249");
}
