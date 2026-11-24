//! Deterministic wiremock fault-injection test suite for [`SorobanRpcClient`].
//!
//! Hardens JSON-RPC 2.0 response parsing, `getLedgerEntries` field handling,
//! corrupt base64 XDR handling, and HTTP 429/502/503 retry/exhaustion behavior
//! against untrusted or degraded RPC endpoints. All tests run offline via `wiremock`.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::Value;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use sentinel_core::adapters::soroban_rpc::Jitter;
use sentinel_core::adapters::{
    derive_contract_instance_key, extract_instance_storage_breakdown, extract_sac_asset_info,
    extract_wasm_hash_from_instance_entry, RetryPolicy, SorobanRpcClient, MAX_BATCH_REQUESTS,
    MAX_KEYS_PER_REQUEST,
};
use sentinel_core::domain::RiskLevel;
use sentinel_core::error::SentinelError;
use sentinel_core::ports::LedgerReader;
use sentinel_core::usecases::{fetch_network_limits, ScanContractUseCase, ScanRequest};

fn deterministic_policy(max_retries: u32, base_delay_ms: u64, max_delay_ms: u64) -> RetryPolicy {
    RetryPolicy {
        max_retries,
        base_delay: Duration::from_millis(base_delay_ms),
        max_delay: Duration::from_millis(max_delay_ms),
        jitter: Jitter::None,
        seed: None,
    }
}

fn client_with_policy(server: &MockServer, policy: RetryPolicy) -> SorobanRpcClient {
    SorobanRpcClient::new(server.uri()).with_retry_policy(policy)
}

fn latest_ledger_ok(sequence: u32, protocol_version: u32) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "result": {
            "sequence": sequence,
            "protocolVersion": protocol_version
        }
    }))
}

// ===========================================================================
// 1. Truncated JSON bodies & malformed JSON-RPC envelopes
// ===========================================================================

#[tokio::test]
async fn test_truncated_json_get_latest_ledger_returns_serialization_error_without_retrying() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/json")
                .set_body_string(r#"{"jsonrpc":"2.0","id":1,"result":{"sequence":555000"#),
        )
        .expect(1)
        .mount(&mock_server)
        .await;

    let client = client_with_policy(&mock_server, deterministic_policy(3, 1, 10));
    let err = client
        .get_latest_ledger()
        .await
        .expect_err("Truncated JSON body must return SerializationError");

    assert!(
        matches!(err, SentinelError::SerializationError(ref msg) if msg.contains("Failed to parse JSON-RPC response")),
        "Expected SerializationError for truncated JSON, got: {err:?}"
    );
    mock_server.verify().await;
}

#[tokio::test]
async fn test_truncated_json_get_ledger_entries_mid_array_returns_serialization_error() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/json")
                .set_body_string(
                    r#"{"jsonrpc":"2.0","id":1,"result":{"latestLedger":5400000,"entries":[{"key":"K1","xdr":"AAAA""#,
                ),
        )
        .expect(1)
        .mount(&mock_server)
        .await;

    let client = client_with_policy(&mock_server, deterministic_policy(3, 1, 10));
    let err = client
        .get_ledger_entries(&["K1".to_string()])
        .await
        .expect_err("Truncated getLedgerEntries payload must fail");

    assert!(
        matches!(err, SentinelError::SerializationError(_)),
        "Expected SerializationError, got: {err:?}"
    );
    mock_server.verify().await;
}

#[tokio::test]
async fn test_empty_http_200_body_returns_serialization_error() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(""))
        .expect(1)
        .mount(&mock_server)
        .await;

    let client = client_with_policy(&mock_server, deterministic_policy(3, 1, 10));
    let err = client
        .get_latest_ledger()
        .await
        .expect_err("Empty HTTP 200 body must return SerializationError");

    assert!(
        matches!(err, SentinelError::SerializationError(_)),
        "Expected SerializationError for empty body, got: {err:?}"
    );
    mock_server.verify().await;
}

#[tokio::test]
async fn test_valid_json_missing_result_and_error_returns_rpc_response_error() {
    for payload in [
        serde_json::json!({ "jsonrpc": "2.0", "id": 1 }),
        serde_json::json!({ "jsonrpc": "2.0", "id": 1, "result": null }),
    ] {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(payload))
            .expect(1)
            .mount(&mock_server)
            .await;

        let client = client_with_policy(&mock_server, deterministic_policy(3, 1, 10));
        let err = client
            .get_latest_ledger()
            .await
            .expect_err("JSON-RPC response without result or error must fail");

        assert!(
            matches!(
                err,
                SentinelError::RpcResponseError {
                    code: -32603,
                    ref message
                } if message.contains("Missing result field")
            ),
            "Expected RpcResponseError(-32603) for missing result, got: {err:?}"
        );
        mock_server.verify().await;
    }
}

// ===========================================================================
// 2. Non-JSON HTML 502/503 error pages & non-retryable HTTP status codes
// ===========================================================================

#[tokio::test]
async fn test_html_502_bad_gateway_fails_fast_without_retrying() {
    let mock_server = MockServer::start().await;

    let html_502 = "<!DOCTYPE html><html><head><title>502 Bad Gateway</title></head>\
                    <body><h1>502 Bad Gateway</h1><hr><center>nginx/1.25.0</center></body></html>";

    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(
            ResponseTemplate::new(502)
                .insert_header("content-type", "text/html; charset=utf-8")
                .set_body_string(html_502),
        )
        .expect(1)
        .mount(&mock_server)
        .await;

    let client = client_with_policy(&mock_server, deterministic_policy(3, 1, 10));
    let err = client
        .get_latest_ledger()
        .await
        .expect_err("HTTP 502 HTML page must return RpcResponseError");

    assert!(
        matches!(
            err,
            SentinelError::RpcResponseError {
                code: 502,
                ref message
            } if message.contains("502")
        ),
        "Expected RpcResponseError with code 502, got: {err:?}"
    );
    mock_server.verify().await;
}

#[tokio::test]
async fn test_html_503_service_unavailable_exhausts_retries_and_returns_typed_error() {
    let mock_server = MockServer::start().await;

    let html_503 = "<!DOCTYPE html><html><body><h1>503 Service Temporarily Unavailable</h1>\
                    <p>Upstream Soroban RPC pool overloaded</p></body></html>";

    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(
            ResponseTemplate::new(503)
                .insert_header("content-type", "text/html")
                .set_body_string(html_503),
        )
        .expect(3)
        .mount(&mock_server)
        .await;

    let client = client_with_policy(&mock_server, deterministic_policy(3, 2, 10));
    let err = client
        .get_ledger_entries(&["TEST_KEY".to_string()])
        .await
        .expect_err("Persistent HTML 503 must exhaust retries and fail");

    assert!(
        matches!(
            err,
            SentinelError::RpcResponseError {
                code: 503,
                ref message
            } if message.contains("503")
        ),
        "Expected RpcResponseError with code 503, got: {err:?}"
    );
    mock_server.verify().await;
}

#[tokio::test]
async fn test_http_200_html_error_page_returns_serialization_error() {
    let mock_server = MockServer::start().await;

    let html_body = "<!DOCTYPE html><html><body><h1>Maintenance Mode</h1></body></html>";

    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/html")
                .set_body_string(html_body),
        )
        .expect(1)
        .mount(&mock_server)
        .await;

    let client = client_with_policy(&mock_server, deterministic_policy(3, 1, 10));
    let err = client
        .get_latest_ledger()
        .await
        .expect_err("HTML body on HTTP 200 must return SerializationError");

    assert!(
        matches!(err, SentinelError::SerializationError(_)),
        "Expected SerializationError for HTML 200 body, got: {err:?}"
    );
    mock_server.verify().await;
}

#[tokio::test]
async fn test_non_retryable_http_4xx_statuses_fail_immediately() {
    for status_code in [400u16, 401, 403, 404] {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/"))
            .respond_with(
                ResponseTemplate::new(status_code)
                    .insert_header("content-type", "text/html")
                    .set_body_string(format!("<html><body>HTTP {status_code}</body></html>")),
            )
            .expect(1)
            .mount(&mock_server)
            .await;

        let client = client_with_policy(&mock_server, deterministic_policy(3, 1, 10));
        let err = client
            .get_latest_ledger()
            .await
            .expect_err("Non-retryable 4xx status must fail immediately");

        assert!(
            matches!(
                err,
                SentinelError::RpcResponseError { code, .. } if code == i64::from(status_code)
            ),
            "Expected RpcResponseError with code {status_code}, got: {err:?}"
        );
        mock_server.verify().await;
    }
}

// ===========================================================================
// 3. JSON-RPC error objects & mismatched JSON-RPC IDs
// ===========================================================================

#[tokio::test]
async fn test_jsonrpc_error_objects_preserve_code_and_message_without_retrying() {
    let cases = [
        (-32700i64, "Parse error", None),
        (-32600, "Invalid Request", None),
        (-32601, "Method not found", None),
        (
            -32602,
            "Invalid params",
            Some(serde_json::json!({"param": "keys", "reason": "malformed XDR"})),
        ),
        (-32603, "Internal error", None),
        (
            -32000,
            "Ledger sequence out of retention window",
            Some(serde_json::json!({"oldestLedger": 5000000})),
        ),
    ];

    for (expected_code, expected_message, data) in cases {
        let mock_server = MockServer::start().await;

        let mut error_obj = serde_json::json!({
            "code": expected_code,
            "message": expected_message
        });
        if let Some(extra) = data {
            error_obj["data"] = extra;
        }

        Mock::given(method("POST"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "error": error_obj
            })))
            .expect(1)
            .mount(&mock_server)
            .await;

        let client = client_with_policy(&mock_server, deterministic_policy(3, 1, 10));
        let err = client
            .get_ledger_entries(&["TEST_KEY".to_string()])
            .await
            .expect_err("JSON-RPC error object must propagate as RpcResponseError");

        assert_eq!(
            err,
            SentinelError::RpcResponseError {
                code: expected_code,
                message: expected_message.to_string(),
            }
        );
        mock_server.verify().await;
    }
}

#[tokio::test]
async fn test_jsonrpc_response_with_both_error_and_result_returns_error() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "sequence": 5400000,
                "protocolVersion": 22
            },
            "error": {
                "code": -32001,
                "message": "Ambiguous upstream fault"
            }
        })))
        .expect(1)
        .mount(&mock_server)
        .await;

    let client = client_with_policy(&mock_server, deterministic_policy(2, 1, 10));
    let err = client
        .get_latest_ledger()
        .await
        .expect_err("Response containing both error and result must surface error");

    assert_eq!(
        err,
        SentinelError::RpcResponseError {
            code: -32001,
            message: "Ambiguous upstream fault".to_string(),
        }
    );
    mock_server.verify().await;
}

#[tokio::test]
async fn test_malformed_jsonrpc_error_object_returns_serialization_error() {
    for malformed_error in [
        serde_json::json!({ "code": "not-an-integer", "message": "bad code" }),
        serde_json::json!({ "code": -32600 }),
        serde_json::json!("raw-string-error"),
    ] {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "error": malformed_error
            })))
            .expect(1)
            .mount(&mock_server)
            .await;

        let client = client_with_policy(&mock_server, deterministic_policy(2, 1, 10));
        let err = client
            .get_latest_ledger()
            .await
            .expect_err("Malformed JSON-RPC error object must return SerializationError");

        assert!(
            matches!(err, SentinelError::SerializationError(_)),
            "Expected SerializationError, got: {err:?}"
        );
        mock_server.verify().await;
    }
}

#[tokio::test]
async fn test_mismatched_and_non_standard_jsonrpc_ids_handled_deterministically() {
    // Request always sends id = 1; verify client handles mismatched numeric, string, null,
    // and omitted response IDs deterministically without panicking.
    let id_variants: Vec<Option<Value>> = vec![
        Some(serde_json::json!(999)),
        Some(serde_json::json!("wrong-correlation-id")),
        Some(Value::Null),
        None,
    ];

    for (idx, response_id) in id_variants.into_iter().enumerate() {
        let mock_server = MockServer::start().await;
        let expected_seq = 5_500_000 + idx as u32;

        Mock::given(method("POST"))
            .and(path("/"))
            .respond_with(move |req: &wiremock::Request| {
                let req_body: Value =
                    serde_json::from_slice(&req.body).expect("Request should be valid JSON");
                assert_eq!(req_body["jsonrpc"], "2.0");
                assert_eq!(req_body["id"], 1);

                let mut resp = serde_json::json!({
                    "jsonrpc": "2.0",
                    "result": {
                        "sequence": expected_seq,
                        "protocolVersion": 22
                    }
                });
                if let Some(id_val) = &response_id {
                    resp["id"] = id_val.clone();
                }
                ResponseTemplate::new(200).set_body_json(resp)
            })
            .expect(1)
            .mount(&mock_server)
            .await;

        let client = client_with_policy(&mock_server, deterministic_policy(1, 1, 10));
        let info = client
            .get_latest_ledger()
            .await
            .expect("Client should parse valid result deterministically across ID shapes");

        assert_eq!(info.sequence, expected_seq);
        assert_eq!(info.protocol_version, 22);
        mock_server.verify().await;
    }
}

// ===========================================================================
// 4. Partial / missing fields in getLedgerEntries responses
// ===========================================================================

#[tokio::test]
async fn test_get_ledger_entries_omitted_null_and_empty_entries_arrays() {
    let result_variants = [
        serde_json::json!({ "latestLedger": 5400000 }),
        serde_json::json!({ "latestLedger": 5400000, "entries": null }),
        serde_json::json!({ "latestLedger": 5400000, "entries": [] }),
    ];

    for result_obj in result_variants {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": result_obj
            })))
            .expect(1)
            .mount(&mock_server)
            .await;

        let client = client_with_policy(&mock_server, deterministic_policy(1, 1, 10));
        let entries = client
            .get_ledger_entries(&["MISSING_KEY".to_string()])
            .await
            .expect("Omitted/null/empty entries array should return empty Vec");

        assert!(entries.is_empty());
        mock_server.verify().await;
    }
}

#[tokio::test]
async fn test_get_ledger_entries_partial_entries_and_missing_optional_fields() {
    let mock_server = MockServer::start().await;

    // Client requests 3 keys, server returns only 2:
    // - KEY_A omits both liveUntilLedgerSeq and lastModifiedLedgerSeq
    // - KEY_B is absent (archived/pruned)
    // - KEY_C uses legacy "entry" alias and sets liveUntilLedgerSeq to null
    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "latestLedger": 5400000,
                "entries": [
                    {
                        "key": "KEY_A",
                        "xdr": "XDR_A"
                    },
                    {
                        "key": "KEY_C",
                        "entry": "XDR_C",
                        "liveUntilLedgerSeq": null,
                        "lastModifiedLedgerSeq": 5395000
                    }
                ]
            }
        })))
        .expect(1)
        .mount(&mock_server)
        .await;

    let client = client_with_policy(&mock_server, deterministic_policy(1, 1, 10));
    let entries = client
        .get_ledger_entries(&[
            "KEY_A".to_string(),
            "KEY_B".to_string(),
            "KEY_C".to_string(),
        ])
        .await
        .expect("Partial entries response with missing optional fields should succeed");

    assert_eq!(entries.len(), 2);

    assert_eq!(entries[0].key_xdr_base64, "KEY_A");
    assert_eq!(entries[0].entry_xdr_base64, "XDR_A");
    assert_eq!(entries[0].live_until_ledger_seq, 0);
    assert_eq!(entries[0].last_modified_ledger_seq, 0);

    assert_eq!(entries[1].key_xdr_base64, "KEY_C");
    assert_eq!(entries[1].entry_xdr_base64, "XDR_C");
    assert_eq!(entries[1].live_until_ledger_seq, 0);
    assert_eq!(entries[1].last_modified_ledger_seq, 5_395_000);

    mock_server.verify().await;
}

#[tokio::test]
async fn test_get_ledger_entries_missing_required_fields_or_wrong_types_fail() {
    let malformed_results = [
        // Missing required "key"
        serde_json::json!({
            "latestLedger": 5400000,
            "entries": [{ "xdr": "XDR_ONLY", "liveUntilLedgerSeq": 5500000 }]
        }),
        // Missing required "xdr" (and "entry" alias)
        serde_json::json!({
            "latestLedger": 5400000,
            "entries": [{ "key": "KEY_ONLY", "liveUntilLedgerSeq": 5500000 }]
        }),
        // Wrong type for liveUntilLedgerSeq (string instead of u32)
        serde_json::json!({
            "latestLedger": 5400000,
            "entries": [{ "key": "K", "xdr": "X", "liveUntilLedgerSeq": "5500000" }]
        }),
        // Negative liveUntilLedgerSeq
        serde_json::json!({
            "latestLedger": 5400000,
            "entries": [{ "key": "K", "xdr": "X", "liveUntilLedgerSeq": -1 }]
        }),
        // Wrong type for entries (object instead of array)
        serde_json::json!({
            "latestLedger": 5400000,
            "entries": { "key": "K", "xdr": "X" }
        }),
    ];

    for bad_result in malformed_results {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": bad_result
            })))
            .expect(1)
            .mount(&mock_server)
            .await;

        let client = client_with_policy(&mock_server, deterministic_policy(3, 1, 10));
        let err = client
            .get_ledger_entries(&["K".to_string()])
            .await
            .expect_err("Malformed getLedgerEntries item must return SerializationError");

        assert!(
            matches!(err, SentinelError::SerializationError(_)),
            "Expected SerializationError, got: {err:?}"
        );
        mock_server.verify().await;
    }
}

#[tokio::test]
async fn test_get_ledger_entries_second_chunk_fault_propagates_typed_error() {
    let mock_server = MockServer::start().await;
    let call_count = Arc::new(AtomicU32::new(0));
    let call_count_clone = Arc::clone(&call_count);

    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(move |req: &wiremock::Request| {
            let batch_num = call_count_clone.fetch_add(1, Ordering::SeqCst) + 1;
            let requests: Vec<Value> =
                serde_json::from_slice(&req.body).expect("Valid JSON-RPC batch request");

            if batch_num == 1 {
                assert_eq!(requests.len(), MAX_BATCH_REQUESTS);
                let responses: Vec<Value> = requests
                    .iter()
                    .map(|request| {
                        let keys = request["params"]["keys"]
                            .as_array()
                            .expect("keys array in params");
                        assert_eq!(keys.len(), MAX_KEYS_PER_REQUEST);
                        serde_json::json!({
                            "jsonrpc": "2.0",
                            "id": request["id"],
                            "result": {
                                "latestLedger": 5400000,
                                "entries": [{
                                    "key": keys[0],
                                    "xdr": "VALID_PLACEHOLDER",
                                    "liveUntilLedgerSeq": 5600000
                                }]
                            }
                        })
                    })
                    .collect();
                ResponseTemplate::new(200).set_body_json(responses)
            } else {
                // Second batch returns truncated JSON
                assert_eq!(requests.len(), 1);
                ResponseTemplate::new(200)
                    .set_body_string(r#"[{"jsonrpc":"2.0","id":21,"result":{"entries":[{"key":"#)
            }
        })
        .expect(2)
        .mount(&mock_server)
        .await;

    let keys: Vec<String> = (0..MAX_KEYS_PER_REQUEST * MAX_BATCH_REQUESTS + 5)
        .map(|i| format!("CHUNK_KEY_{i}"))
        .collect();
    let client = client_with_policy(&mock_server, deterministic_policy(2, 1, 10));

    let err = client
        .get_ledger_entries(&keys)
        .await
        .expect_err("Fault in second batch must fail entire get_ledger_entries call");

    assert!(
        matches!(err, SentinelError::SerializationError(_)),
        "Expected SerializationError from second batch, got: {err:?}"
    );
    mock_server.verify().await;
}

// ===========================================================================
// 5. Corrupt base64 XDR strings
// ===========================================================================

#[tokio::test]
async fn test_corrupt_base64_xdr_in_get_ledger_entries_rejected_by_xdr_extractors_and_safe_in_scan()
{
    let mock_server = MockServer::start().await;

    let contract_id = [42u8; 32];
    let contract_strkey = stellar_strkey::Contract(contract_id).to_string();
    let instance_key = derive_contract_instance_key(&contract_id).expect("valid instance key");
    let corrupt_xdr_invalid_b64 = "!!!NOT_VALID_BASE64_XDR@@@";
    let corrupt_xdr_truncated_b64 = "AAAA"; // Valid base64, invalid/truncated LedgerEntry XDR

    let instance_key_clone = instance_key.clone();
    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(move |req: &wiremock::Request| {
            let body: Value = serde_json::from_slice(&req.body).expect("Valid JSON");
            match body["method"].as_str() {
                Some("getLatestLedger") => latest_ledger_ok(5_400_000, 22),
                Some("getLedgers") => ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "error": { "code": -32601, "message": "Method not found" }
                })),
                Some("getLedgerEntries") => {
                    ResponseTemplate::new(200).set_body_json(serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": 1,
                        "result": {
                            "latestLedger": 5400000,
                            "entries": [
                                {
                                    "key": instance_key_clone,
                                    "xdr": corrupt_xdr_invalid_b64,
                                    "liveUntilLedgerSeq": 5600000,
                                    "lastModifiedLedgerSeq": 5390000
                                },
                                {
                                    "key": "EXTRA_CORRUPT_KEY",
                                    "xdr": corrupt_xdr_truncated_b64,
                                    "liveUntilLedgerSeq": 5600000,
                                    "lastModifiedLedgerSeq": 5390000
                                }
                            ]
                        }
                    }))
                }
                other => panic!("Unexpected RPC method: {other:?}"),
            }
        })
        .mount(&mock_server)
        .await;

    let client = client_with_policy(&mock_server, deterministic_policy(1, 1, 10));

    // 1. Direct RPC transport returns the raw corrupt strings without panicking,
    //    and XDR extractors return typed SentinelError::InvalidXdrKey.
    let raw_entries = client
        .get_ledger_entries(std::slice::from_ref(&instance_key))
        .await
        .expect("Transport should deliver raw entries");
    assert_eq!(raw_entries.len(), 2);

    for raw in &raw_entries {
        assert!(matches!(
            extract_wasm_hash_from_instance_entry(&raw.entry_xdr_base64),
            Err(SentinelError::InvalidXdrKey(_))
        ));
        assert!(matches!(
            extract_instance_storage_breakdown(&raw.entry_xdr_base64),
            Err(SentinelError::InvalidXdrKey(_))
        ));
        assert!(matches!(
            extract_sac_asset_info(&raw.entry_xdr_base64),
            Err(SentinelError::InvalidXdrKey(_))
        ));
    }

    // 2. End-to-end scan over the same corrupt wiremock response does not panic and
    //    gracefully degrades (no WASM code lookup, no SAC metadata, fallback byte size).
    let usecase = ScanContractUseCase::new(client);
    let report = usecase
        .execute(ScanRequest::new(&contract_strkey))
        .await
        .expect("Scan must handle corrupt base64 XDR gracefully without panicking");

    assert_eq!(report.entries.len(), 1);
    assert_eq!(report.entries[0].key_identifier, "instance");
    assert_eq!(report.entries[0].status, RiskLevel::Ok);
    assert!(report.entries[0].byte_size > 0);
    assert!(report.entries[0].instance_storage_keys.is_empty());
    assert!(report.sac_asset_info.is_none());
}

#[tokio::test]
async fn test_corrupt_base64_xdr_in_network_limits_returns_malformed_network_config() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(|req: &wiremock::Request| {
            let body: Value = serde_json::from_slice(&req.body).expect("Valid JSON");
            match body["method"].as_str() {
                Some("getLatestLedger") => latest_ledger_ok(5_400_000, 22),
                Some("getLedgerEntries") => {
                    ResponseTemplate::new(200).set_body_json(serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": 1,
                        "result": {
                            "latestLedger": 5400000,
                            "entries": [{
                                "key": "CONFIG_KEY",
                                "xdr": "%%%CORRUPT_BASE64_CONFIG_XDR%%%",
                                "liveUntilLedgerSeq": 0
                            }]
                        }
                    }))
                }
                other => panic!("Unexpected RPC method: {other:?}"),
            }
        })
        .expect(2)
        .mount(&mock_server)
        .await;

    let client = client_with_policy(&mock_server, deterministic_policy(1, 1, 10));
    let err = fetch_network_limits(&client)
        .await
        .expect_err("Corrupt base64 config XDR must return MalformedNetworkConfig");

    assert!(
        matches!(err, SentinelError::MalformedNetworkConfig(_)),
        "Expected MalformedNetworkConfig, got: {err:?}"
    );
    mock_server.verify().await;
}

#[tokio::test]
async fn test_corrupt_base64_xdr_in_simulate_transaction_returns_serialization_error() {
    for corrupt_tx_data in ["!!!NOT_BASE64!!!", "AAAA", "dGVzdA=="] {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": {
                    "minResourceFee": "12345",
                    "latestLedger": 5400000,
                    "transactionData": corrupt_tx_data
                }
            })))
            .expect(1)
            .mount(&mock_server)
            .await;

        let client = client_with_policy(&mock_server, deterministic_policy(3, 1, 10));
        let err = client
            .simulate_transaction("tx-xdr", "footprint-xdr", 5_400_000)
            .await
            .expect_err("Corrupt transactionData XDR must fail with SerializationError");

        assert!(
            matches!(
                err,
                SentinelError::SerializationError(ref msg) if msg.contains("Invalid transactionData")
            ),
            "Expected SerializationError for corrupt transactionData '{corrupt_tx_data}', got: {err:?}"
        );
        mock_server.verify().await;
    }
}

#[tokio::test]
async fn test_simulate_transaction_invalid_min_resource_fee_returns_serialization_error() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "minResourceFee": "not-a-u64",
                "latestLedger": 5400000
            }
        })))
        .expect(1)
        .mount(&mock_server)
        .await;

    let client = client_with_policy(&mock_server, deterministic_policy(2, 1, 10));
    let err = client
        .simulate_transaction("tx-xdr", "footprint-xdr", 5_400_000)
        .await
        .expect_err("Non-numeric minResourceFee must return SerializationError");

    assert!(
        matches!(
            err,
            SentinelError::SerializationError(ref msg) if msg.contains("Invalid minResourceFee")
        ),
        "Expected SerializationError for invalid minResourceFee, got: {err:?}"
    );
    mock_server.verify().await;
}

// ===========================================================================
// 6. HTTP 429 with numeric Retry-After, invalid Retry-After, and exhaustion
// ===========================================================================

#[tokio::test]
async fn test_http_429_numeric_retry_after_takes_precedence_over_base_delay() {
    let mock_server = MockServer::start().await;
    let call_count = Arc::new(AtomicU32::new(0));
    let call_count_clone = Arc::clone(&call_count);

    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(move |_: &wiremock::Request| {
            let attempt = call_count_clone.fetch_add(1, Ordering::SeqCst) + 1;
            if attempt == 1 {
                // Positive numeric Retry-After (1s) padded with whitespace.
                // Clamped by max_delay (30ms), which is still well above base_delay (1ms).
                ResponseTemplate::new(429).append_header("Retry-After", " 1 ")
            } else {
                latest_ledger_ok(7_100_000, 22)
            }
        })
        .expect(2)
        .mount(&mock_server)
        .await;

    let policy = deterministic_policy(2, 1, 30);
    let client = client_with_policy(&mock_server, policy);

    let start = Instant::now();
    let info = client
        .get_latest_ledger()
        .await
        .expect("Should recover on attempt 2 after numeric Retry-After");
    let elapsed = start.elapsed();

    assert_eq!(info.sequence, 7_100_000);
    assert_eq!(call_count.load(Ordering::SeqCst), 2);
    // Clamped Retry-After sleeps for 30ms instead of 1ms base_delay.
    assert!(
        elapsed >= Duration::from_millis(20),
        "Expected numeric Retry-After delay (>=20ms), elapsed: {elapsed:?}"
    );
    assert!(
        elapsed < Duration::from_secs(5),
        "Expected Retry-After to remain clamped to max_delay, elapsed: {elapsed:?}"
    );
    mock_server.verify().await;
}

#[tokio::test]
async fn test_http_429_invalid_retry_after_headers_fallback_to_backoff_and_recover() {
    let invalid_headers = [
        "not-a-number",
        "Wed, 21 Oct 2015 07:28:00 GMT",
        "-5",
        "",
        "1.5",
        "999999999999999999999999999999",
    ];

    for header_val in invalid_headers {
        let mock_server = MockServer::start().await;
        let call_count = Arc::new(AtomicU32::new(0));
        let call_count_clone = Arc::clone(&call_count);
        let header_owned = header_val.to_string();

        Mock::given(method("POST"))
            .and(path("/"))
            .respond_with(move |_: &wiremock::Request| {
                let attempt = call_count_clone.fetch_add(1, Ordering::SeqCst) + 1;
                if attempt == 1 {
                    ResponseTemplate::new(429).append_header("Retry-After", header_owned.as_str())
                } else {
                    latest_ledger_ok(7_200_000, 22)
                }
            })
            .expect(2)
            .mount(&mock_server)
            .await;

        let client = client_with_policy(&mock_server, deterministic_policy(3, 2, 50));
        let info = client.get_latest_ledger().await.unwrap_or_else(|e| {
            panic!("Invalid Retry-After '{header_val}' should fall back to backoff, got: {e:?}")
        });

        assert_eq!(info.sequence, 7_200_000);
        assert_eq!(call_count.load(Ordering::SeqCst), 2);
        mock_server.verify().await;
    }
}

#[tokio::test]
async fn test_http_429_retry_exhaustion_with_numeric_retry_after_returns_typed_error() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(
            ResponseTemplate::new(429)
                .append_header("Retry-After", "0")
                .set_body_string("Too Many Requests"),
        )
        .expect(3)
        .mount(&mock_server)
        .await;

    let client = client_with_policy(&mock_server, deterministic_policy(3, 2, 20));
    let err = client
        .get_latest_ledger()
        .await
        .expect_err("Persistent HTTP 429 must exhaust retries and return SentinelError");

    assert!(
        matches!(
            err,
            SentinelError::RpcResponseError {
                code: 429,
                ref message
            } if message.contains("429")
        ),
        "Expected RpcResponseError with code 429, got: {err:?}"
    );
    mock_server.verify().await;
}

#[tokio::test]
async fn test_http_429_retry_exhaustion_with_invalid_retry_after_returns_typed_error() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(
            ResponseTemplate::new(429)
                .append_header("Retry-After", "invalid-retry-after-value")
                .insert_header("content-type", "text/html")
                .set_body_string("<html><body>429 Rate Limit Exceeded</body></html>"),
        )
        .expect(3)
        .mount(&mock_server)
        .await;

    let client = client_with_policy(&mock_server, deterministic_policy(3, 2, 20));
    let err = client
        .get_ledger_entries(&["RATE_LIMITED_KEY".to_string()])
        .await
        .expect_err("Exhausted 429 with invalid Retry-After must return typed error");

    assert!(
        matches!(
            err,
            SentinelError::RpcResponseError {
                code: 429,
                ref message
            } if message.contains("429")
        ),
        "Expected RpcResponseError with code 429, got: {err:?}"
    );
    mock_server.verify().await;
}

#[tokio::test]
async fn test_mixed_429_and_503_faults_exhausting_retries_returns_final_typed_error() {
    let mock_server = MockServer::start().await;
    let call_count = Arc::new(AtomicU32::new(0));
    let call_count_clone = Arc::clone(&call_count);

    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(move |_: &wiremock::Request| {
            let attempt = call_count_clone.fetch_add(1, Ordering::SeqCst) + 1;
            match attempt {
                1 => ResponseTemplate::new(429).append_header("Retry-After", "bogus"),
                2 => ResponseTemplate::new(503)
                    .insert_header("content-type", "text/html")
                    .set_body_string("<html><body>503 Overloaded</body></html>"),
                _ => ResponseTemplate::new(429).append_header("Retry-After", "0"),
            }
        })
        .expect(3)
        .mount(&mock_server)
        .await;

    let client = client_with_policy(&mock_server, deterministic_policy(3, 2, 20));
    let err = client
        .get_latest_ledger()
        .await
        .expect_err("Mixed transient faults exhausting max_retries must return final error");

    assert!(
        matches!(err, SentinelError::RpcResponseError { code: 429, .. }),
        "Expected final RpcResponseError(429), got: {err:?}"
    );
    assert_eq!(call_count.load(Ordering::SeqCst), 3);
    mock_server.verify().await;
}

// ===========================================================================
// 7. Fault tolerance for optional getLedgers velocity queries
// ===========================================================================

#[tokio::test]
async fn test_get_ledger_close_velocity_faults_fallback_safely_to_none() {
    let fault_responses = [
        ResponseTemplate::new(200)
            .set_body_string(r#"{"jsonrpc":"2.0","id":1,"result":{"ledgers":["#),
        ResponseTemplate::new(502)
            .insert_header("content-type", "text/html")
            .set_body_string("<html><body>502 Bad Gateway</body></html>"),
        ResponseTemplate::new(429).append_header("Retry-After", "invalid"),
    ];

    for response in fault_responses {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/"))
            .respond_with(response)
            .mount(&mock_server)
            .await;

        let client = client_with_policy(&mock_server, deterministic_policy(2, 1, 10));
        let velocity = client
            .get_ledger_close_velocity(5_400_000)
            .await
            .expect("get_ledger_close_velocity must gracefully fall back to Ok(None) on faults");

        assert_eq!(velocity, None);
    }
}
