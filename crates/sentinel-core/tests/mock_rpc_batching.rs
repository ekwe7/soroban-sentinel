use sentinel_core::adapters::{SorobanRpcClient, MAX_BATCH_REQUESTS, MAX_KEYS_PER_REQUEST};
use sentinel_core::ports::LedgerReader;
use serde_json::{json, Value};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

#[tokio::test]
async fn get_ledger_entries_batches_chunks_and_correlates_responses_by_id() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(|request: &Request| {
            let requests: Vec<Value> = serde_json::from_slice(&request.body).unwrap();
            assert_eq!(requests.len(), 2);
            assert_eq!(requests[0]["method"], "getLedgerEntries");
            assert_eq!(
                requests[0]["params"]["keys"].as_array().unwrap().len(),
                MAX_KEYS_PER_REQUEST
            );
            assert_eq!(requests[1]["params"]["keys"].as_array().unwrap().len(), 50);

            let mut responses = requests
                .iter()
                .map(|rpc_request| {
                    let id = rpc_request["id"].as_u64().unwrap();
                    let entries: Vec<Value> = rpc_request["params"]["keys"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|key| {
                            let key = key.as_str().unwrap();
                            json!({
                                "key": key,
                                "xdr": format!("XDR_FOR_{key}"),
                                "liveUntilLedgerSeq": 6_000_000,
                                "lastModifiedLedgerSeq": 5_900_000
                            })
                        })
                        .collect();
                    json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": { "entries": entries }
                    })
                })
                .collect::<Vec<_>>();
            responses.reverse();
            ResponseTemplate::new(200).set_body_json(responses)
        })
        .expect(1)
        .mount(&server)
        .await;

    let keys = (0..250)
        .map(|i| format!("TEST_KEY_{i}"))
        .collect::<Vec<_>>();
    let entries = SorobanRpcClient::new(server.uri())
        .get_ledger_entries(&keys)
        .await
        .expect("the batch should succeed");

    assert_eq!(entries.len(), keys.len());
    for (index, entry) in entries.iter().enumerate() {
        assert_eq!(entry.key_xdr_base64, format!("TEST_KEY_{index}"));
        assert_eq!(entry.entry_xdr_base64, format!("XDR_FOR_TEST_KEY_{index}"));
    }

    let received = server.received_requests().await.unwrap();
    assert_eq!(received.len(), 1);
    let payload: Value = serde_json::from_slice(&received[0].body).unwrap();
    assert_eq!(payload.as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn get_ledger_entries_keeps_successful_items_when_one_batch_item_fails() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(|request: &Request| {
            let requests: Vec<Value> = serde_json::from_slice(&request.body).unwrap();
            assert_eq!(requests.len(), 2);
            ResponseTemplate::new(200).set_body_json(vec![
                json!({
                    "jsonrpc": "2.0",
                    "id": 2,
                    "result": {
                        "entries": [{
                            "key": "TEST_KEY_200",
                            "xdr": "XDR_FOR_TEST_KEY_200",
                            "liveUntilLedgerSeq": 6_000_000,
                            "lastModifiedLedgerSeq": 5_900_000
                        }]
                    }
                }),
                json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "error": { "code": -32000, "message": "temporary item failure" }
                }),
            ])
        })
        .expect(1)
        .mount(&server)
        .await;

    let keys = (0..201)
        .map(|i| format!("TEST_KEY_{i}"))
        .collect::<Vec<_>>();
    let entries = SorobanRpcClient::new(server.uri())
        .get_ledger_entries(&keys)
        .await
        .expect("an item error must not fail the whole batch");

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].key_xdr_base64, "TEST_KEY_200");
}

#[tokio::test]
async fn get_ledger_entries_respects_maximum_batch_size() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(|request: &Request| {
            let requests: Vec<Value> = serde_json::from_slice(&request.body).unwrap();
            assert!(requests.len() <= MAX_BATCH_REQUESTS);
            assert!(requests
                .iter()
                .all(
                    |rpc_request| rpc_request["params"]["keys"].as_array().unwrap().len()
                        <= MAX_KEYS_PER_REQUEST
                ));
            let responses = requests
                .iter()
                .map(|rpc_request| {
                    json!({
                        "jsonrpc": "2.0",
                        "id": rpc_request["id"],
                        "result": { "entries": [] }
                    })
                })
                .collect::<Vec<_>>();
            ResponseTemplate::new(200).set_body_json(responses)
        })
        .expect(2)
        .mount(&server)
        .await;

    let key_count = MAX_KEYS_PER_REQUEST * (MAX_BATCH_REQUESTS + 1);
    let keys = (0..key_count)
        .map(|i| format!("TEST_KEY_{i}"))
        .collect::<Vec<_>>();
    let entries = SorobanRpcClient::new(server.uri())
        .get_ledger_entries(&keys)
        .await
        .expect("bounded batch requests should succeed");

    assert!(entries.is_empty());
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
}
