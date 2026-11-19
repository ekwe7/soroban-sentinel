use serde_json::Value;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use sentinel_core::adapters::{SorobanRpcClient, MAX_KEYS_PER_REQUEST, MAX_RPC_BATCH_SIZE};
use sentinel_core::ports::LedgerReader;

#[tokio::test]
async fn batches_requests_and_returns_entries_in_request_order() {
    let mock_server = MockServer::start().await;
    let keys: Vec<String> = (0..250).map(|i| format!("KEY_{i}")).collect();

    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(|request: &wiremock::Request| {
            let batch: Vec<Value> =
                serde_json::from_slice(&request.body).expect("Valid JSON-RPC batch");
            assert_eq!(batch.len(), 2);
            assert!(batch
                .iter()
                .all(|call| call["method"] == "getLedgerEntries"));
            assert!(batch.iter().all(|call| {
                call["params"]["keys"].as_array().unwrap().len() <= MAX_KEYS_PER_REQUEST
            }));

            let responses: Vec<Value> = batch
                .iter()
                .rev()
                .map(|call| {
                    let entries: Vec<Value> = call["params"]["keys"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|key| {
                            let key = key.as_str().unwrap();
                            serde_json::json!({
                                "key": key,
                                "xdr": format!("ENTRY_{key}"),
                                "liveUntilLedgerSeq": 100,
                                "lastModifiedLedgerSeq": 90
                            })
                        })
                        .collect();
                    serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": call["id"],
                        "result": { "entries": entries }
                    })
                })
                .collect();

            ResponseTemplate::new(200).set_body_json(responses)
        })
        .expect(1)
        .mount(&mock_server)
        .await;

    let client = SorobanRpcClient::new(mock_server.uri());
    let entries = client.get_ledger_entries(&keys).await.unwrap();

    assert_eq!(entries.len(), keys.len());
    for (index, entry) in entries.iter().enumerate() {
        assert_eq!(entry.key_xdr_base64, keys[index]);
        assert_eq!(entry.entry_xdr_base64, format!("ENTRY_{}", keys[index]));
    }

    let requests = mock_server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    let batch: Vec<Value> = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(batch[0]["params"]["keys"].as_array().unwrap().len(), 200);
    assert_eq!(batch[1]["params"]["keys"].as_array().unwrap().len(), 50);
    mock_server.verify().await;
}

#[tokio::test]
async fn continues_when_one_batch_item_returns_an_rpc_error() {
    let mock_server = MockServer::start().await;
    let keys: Vec<String> = (0..401).map(|i| format!("KEY_{i}")).collect();

    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(|request: &wiremock::Request| {
            let batch: Vec<Value> =
                serde_json::from_slice(&request.body).expect("Valid JSON-RPC batch");
            assert_eq!(batch.len(), 3);

            let responses: Vec<Value> = batch
                .iter()
                .rev()
                .map(|call| {
                    let id = call["id"].as_u64().unwrap();
                    if id == 2 {
                        serde_json::json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "error": { "code": -32000, "message": "Chunk failed" }
                        })
                    } else {
                        let entries: Vec<Value> = call["params"]["keys"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .map(|key| {
                                serde_json::json!({
                                    "key": key,
                                    "xdr": format!("ENTRY_{}", key.as_str().unwrap())
                                })
                            })
                            .collect();
                        serde_json::json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "result": { "entries": entries }
                        })
                    }
                })
                .collect();

            ResponseTemplate::new(200).set_body_json(responses)
        })
        .expect(1)
        .mount(&mock_server)
        .await;

    let client = SorobanRpcClient::new(mock_server.uri());
    let entries = client.get_ledger_entries(&keys).await.unwrap();

    assert_eq!(entries.len(), 201);
    assert_eq!(entries[0].key_xdr_base64, "KEY_0");
    assert_eq!(entries[199].key_xdr_base64, "KEY_199");
    assert_eq!(entries[200].key_xdr_base64, "KEY_400");
    mock_server.verify().await;
}

#[tokio::test]
async fn caps_the_number_of_calls_in_each_http_batch() {
    let mock_server = MockServer::start().await;
    let keys: Vec<String> = (0..(MAX_KEYS_PER_REQUEST * (MAX_RPC_BATCH_SIZE + 1)))
        .map(|i| format!("KEY_{i}"))
        .collect();

    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(|request: &wiremock::Request| {
            let batch: Vec<Value> =
                serde_json::from_slice(&request.body).expect("Valid JSON-RPC batch");
            assert!(batch.len() <= MAX_RPC_BATCH_SIZE);
            let responses: Vec<Value> = batch
                .iter()
                .map(|call| {
                    serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": call["id"],
                        "result": { "entries": [] }
                    })
                })
                .collect();
            ResponseTemplate::new(200).set_body_json(responses)
        })
        .expect(2)
        .mount(&mock_server)
        .await;

    let client = SorobanRpcClient::new(mock_server.uri());
    let entries = client.get_ledger_entries(&keys).await.unwrap();
    assert!(entries.is_empty());

    let requests = mock_server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 2);
    let first_batch: Vec<Value> = serde_json::from_slice(&requests[0].body).unwrap();
    let second_batch: Vec<Value> = serde_json::from_slice(&requests[1].body).unwrap();
    assert_eq!(first_batch.len(), MAX_RPC_BATCH_SIZE);
    assert_eq!(second_batch.len(), 1);
    mock_server.verify().await;
}
