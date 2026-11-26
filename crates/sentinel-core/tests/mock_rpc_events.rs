use sentinel_core::adapters::{SorobanRpcClient, MAX_EVENTS_PER_PAGE};
use sentinel_core::domain::{SorobanEventType, StateArchivalEventKind};
use sentinel_core::error::SentinelError;
use sentinel_core::ports::EventStreamReader;
use serde_json::{json, Value};
use stellar_xdr::curr::{Limits, ScSymbol, ScVal, WriteXdr};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

fn symbol(value: &str) -> ScVal {
    ScVal::Symbol(ScSymbol(value.try_into().unwrap()))
}

fn symbol_xdr(value: &str) -> String {
    symbol(value).to_xdr_base64(Limits::none()).unwrap()
}

fn event(
    id: &str,
    ledger: u32,
    event_type: &str,
    topics: Vec<String>,
    value: Option<String>,
    paging_token: &str,
    contract_id: &str,
) -> Value {
    let mut event = json!({
        "type": event_type,
        "ledger": ledger,
        "ledgerCloseTime": format!("{}", 1_700_000_000u64 + u64::from(ledger)),
        "contractId": contract_id,
        "id": id,
        "pagingToken": paging_token,
        "topic": topics,
        "txHash": format!("tx-{id}"),
        "inSuccessfulContractCall": true
    });
    if let Some(value) = value {
        event["value"] = json!(value);
    }
    event
}

#[tokio::test]
async fn paginates_get_events_and_deserializes_state_archival_events() {
    let server = MockServer::start().await;
    let contract_id = stellar_strkey::Contract([1u8; 32]).to_string();
    let contract_for_mock = contract_id.clone();

    let extend_topic = symbol_xdr("extend_contract_data_ttl");
    let archived_topic = symbol_xdr("contract_entry_archived");
    let restored_topic = symbol_xdr("restore_contract_data");
    let ttl_value = symbol_xdr("ttl_extended");

    let first_page = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "result": {
            "latestLedger": 500,
            "cursor": "page-2",
            "events": [
                event(
                    "event-1",
                    101,
                    "system",
                    vec![extend_topic],
                    Some(ttl_value),
                    "token-1",
                    &contract_for_mock
                ),
                event(
                    "event-2",
                    102,
                    "contract",
                    vec![archived_topic],
                    None,
                    "token-2",
                    &contract_for_mock
                )
            ]
        }
    });
    let second_page = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "result": {
            "latestLedger": 500,
            "events": [
                event(
                    "event-3",
                    103,
                    "diagnostic",
                    vec![restored_topic],
                    None,
                    "token-3",
                    &contract_for_mock
                )
            ]
        }
    });
    let empty_page = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "result": {
            "events": []
        }
    });

    Mock::given(method("POST"))
        .respond_with(move |request: &wiremock::Request| {
            let call: Value = serde_json::from_slice(&request.body).unwrap();
            assert_eq!(call["method"], "getEvents");
            assert_eq!(call["params"]["startLedger"], 100);
            assert_eq!(call["params"]["pagination"]["limit"], MAX_EVENTS_PER_PAGE);
            assert_eq!(
                call["params"]["filters"][0]["contractIds"][0],
                contract_for_mock
            );

            match call["params"]["pagination"]["cursor"].as_str() {
                None => ResponseTemplate::new(200).set_body_json(first_page.clone()),
                Some("page-2") => ResponseTemplate::new(200).set_body_json(second_page.clone()),
                // The second page has no result-level cursor, so the client
                // falls back to the final event's paging token.
                Some("token-3") => ResponseTemplate::new(200).set_body_json(empty_page.clone()),
                other => panic!("unexpected cursor: {other:?}"),
            }
        })
        .mount(&server)
        .await;

    let client = SorobanRpcClient::new(server.uri());
    let events = client.get_contract_events(&contract_id, 100).await.unwrap();

    assert_eq!(events.len(), 3);

    let extension = &events[0];
    assert_eq!(extension.id, "event-1");
    assert_eq!(extension.ledger, 101);
    assert_eq!(extension.paging_token.as_deref(), Some("token-1"));
    assert_eq!(extension.event_type, SorobanEventType::System);
    assert_eq!(extension.contract_id.as_deref(), Some(contract_id.as_str()));
    assert_eq!(extension.kind, StateArchivalEventKind::TtlExtension);
    assert!(extension.is_state_archival());
    assert_eq!(extension.topic_symbols, vec!["extend_contract_data_ttl"]);
    assert_eq!(extension.value_symbols, vec!["ttl_extended"]);
    assert!(extension.in_successful_contract_call);
    assert_eq!(extension.tx_hash.as_deref(), Some("tx-event-1"));

    assert_eq!(events[1].id, "event-2");
    assert_eq!(events[1].kind, StateArchivalEventKind::Archived);
    assert_eq!(events[1].event_type, SorobanEventType::Contract);

    assert_eq!(events[2].id, "event-3");
    assert_eq!(events[2].kind, StateArchivalEventKind::Restored);
    assert_eq!(events[2].event_type, SorobanEventType::Diagnostic);

    let requests = server.received_requests().await.unwrap();
    let event_requests: Vec<Value> = requests
        .iter()
        .map(|request| serde_json::from_slice(&request.body).unwrap())
        .filter(|call: &Value| call["method"] == "getEvents")
        .collect();
    assert_eq!(event_requests.len(), 3);
    assert!(event_requests[0]["params"]["pagination"]
        .get("cursor")
        .is_none());
    assert_eq!(
        event_requests[1]["params"]["pagination"]["cursor"],
        "page-2"
    );
    assert_eq!(
        event_requests[2]["params"]["pagination"]["cursor"],
        "token-3"
    );
}

#[tokio::test]
async fn deduplicates_events_repeated_across_pages_and_stops_on_empty_page() {
    let server = MockServer::start().await;
    let contract_id = stellar_strkey::Contract([2u8; 32]).to_string();
    let contract_for_mock = contract_id.clone();
    let extension_topic = symbol_xdr("extend_contract_instance_ttl");

    let page = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "result": {
            "cursor": "next-page",
            "events": [
                event(
                    "event-repeated",
                    201,
                    "system",
                    vec![extension_topic],
                    None,
                    "token-1",
                    &contract_for_mock
                )
            ]
        }
    });
    let empty_page = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "result": {
            "events": []
        }
    });

    Mock::given(method("POST"))
        .respond_with(move |request: &wiremock::Request| {
            let call: Value = serde_json::from_slice(&request.body).unwrap();
            if call["params"]["pagination"].get("cursor").is_none() {
                ResponseTemplate::new(200).set_body_json(page.clone())
            } else {
                ResponseTemplate::new(200).set_body_json(empty_page.clone())
            }
        })
        .mount(&server)
        .await;

    let client = SorobanRpcClient::new(server.uri());
    let events = client.get_contract_events(&contract_id, 200).await.unwrap();

    assert_eq!(events.len(), 1);
    assert_eq!(events[0].id, "event-repeated");
}

#[tokio::test]
async fn tolerates_unknown_event_types_and_malformed_topic_xdr() {
    let server = MockServer::start().await;
    let contract_id = stellar_strkey::Contract([3u8; 32]).to_string();
    let contract_for_mock = contract_id.clone();

    let response = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "result": {
            "events": [
                event(
                    "event-unknown",
                    301,
                    "future-event-type",
                    vec!["not-valid-base64!".to_string()],
                    Some("also-not-valid!".to_string()),
                    "token-unknown",
                    &contract_for_mock
                ),
                event(
                    "event-other",
                    302,
                    "contract",
                    vec![symbol_xdr("transfer")],
                    None,
                    "token-other",
                    &contract_for_mock
                )
            ]
        }
    });

    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(response))
        .mount(&server)
        .await;

    let client = SorobanRpcClient::new(server.uri());
    let events = client.get_contract_events(&contract_id, 300).await.unwrap();

    assert_eq!(events.len(), 2);
    assert_eq!(events[0].event_type, SorobanEventType::Unknown);
    assert_eq!(events[0].kind, StateArchivalEventKind::Other);
    assert!(events[0].topic_symbols.is_empty());
    assert!(events[0].value_symbols.is_empty());

    assert_eq!(events[1].kind, StateArchivalEventKind::Other);
    assert!(!events[1].is_state_archival());
}

#[tokio::test]
async fn propagates_get_events_rpc_errors() {
    let server = MockServer::start().await;
    let contract_id = stellar_strkey::Contract([4u8; 32]).to_string();

    let response = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "error": {
            "code": -32000,
            "message": "startLedger is outside the retained event window"
        }
    });

    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(response))
        .mount(&server)
        .await;

    let client = SorobanRpcClient::new(server.uri());
    let result = client.get_contract_events(&contract_id, 1).await;

    assert!(matches!(
        result,
        Err(SentinelError::RpcResponseError { code: -32000, .. })
    ));
}

#[tokio::test]
async fn rejects_invalid_contract_ids_before_issuing_rpc_requests() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .mount(&server)
        .await;

    let client = SorobanRpcClient::new(server.uri());
    let result = client.get_contract_events("INVALID_CONTRACT_ID", 1).await;

    assert!(matches!(
        result,
        Err(SentinelError::InvalidContractAddress(_))
    ));
    assert!(server.received_requests().await.unwrap().is_empty());
}
