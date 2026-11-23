use std::time::Duration;

use sentinel_core::{
    RetryPolicy, SentinelError, SorobanRpcClient, SubmissionOutcome, SubmissionPolicy,
    TransactionSubmitter,
};
use serde_json::{json, Value};
use stellar_xdr::curr::TransactionResultCode;
use wiremock::matchers::{body_json, method};
use wiremock::{Mock, MockServer, ResponseTemplate};

const ENVELOPE: &str = "AAAAAgAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAGQAAAAAAAAAAQAAAAAAAAAAAAAAAAAAAAAAAAABAAAAAAAAAEAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
const PASSPHRASE: &str = "Test SDF Network ; September 2015";
const HASH: &str = "e1bf19bbd8ad1dd3f3fb2e5c26cbf7b7eea6d3738fd2a0e21b2f1d7c98664c1d";
const ERROR_XDR: &str = "AAAAAAAAAGT////+AAAAAA==";

fn rpc(name: &str, params: Value, result: Value) -> Mock {
    Mock::given(method("POST"))
        .and(body_json(
            json!({"jsonrpc": "2.0", "id": 1, "method": name, "params": params}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "jsonrpc": "2.0", "id": 1, "result": result,
        })))
}

fn policy() -> SubmissionPolicy {
    SubmissionPolicy {
        send_timeout: Duration::from_secs(2),
        poll_timeout: Duration::from_secs(2),
        poll_interval: Duration::from_millis(5),
    }
}

fn client(server: &MockServer) -> SorobanRpcClient {
    SorobanRpcClient::new(server.uri()).with_retry_policy(RetryPolicy {
        base_delay: Duration::from_millis(1),
        ..RetryPolicy::default()
    })
}

#[tokio::test]
async fn submitter_confirms_pending_and_duplicate_and_retries_only_explicit_retry() {
    for initial in ["PENDING", "DUPLICATE", "TRY_AGAIN_LATER"] {
        let server = MockServer::start().await;
        let params = json!({"transaction": ENVELOPE});
        rpc(
            "sendTransaction",
            params.clone(),
            json!({"status": initial}),
        )
        .up_to_n_times(1)
        .expect(1)
        .mount(&server)
        .await;
        if initial == "TRY_AGAIN_LATER" {
            rpc("sendTransaction", params, json!({"status": "PENDING"}))
                .expect(1)
                .mount(&server)
                .await;
        }
        rpc(
            "getTransaction",
            json!({"hash": HASH}),
            json!({"status": "SUCCESS", "ledger": 42}),
        )
        .expect(1)
        .mount(&server)
        .await;
        assert_eq!(
            client(&server)
                .submit(ENVELOPE, PASSPHRASE, policy())
                .await
                .unwrap(),
            SubmissionOutcome::Confirmed {
                ledger: 42,
                tx_hash: HASH.into()
            }
        );
        assert_eq!(
            server.received_requests().await.unwrap().len(),
            if initial == "TRY_AGAIN_LATER" { 3 } else { 2 }
        );
    }
}

#[tokio::test]
async fn submitter_distinguishes_invalid_envelope_rejection_and_execution_failure() {
    let server = MockServer::start().await;
    assert!(matches!(
        client(&server)
            .submit("invalid", PASSPHRASE, policy())
            .await,
        Err(SentinelError::InvalidTransactionEnvelope(_))
    ));
    rpc(
        "sendTransaction",
        json!({"transaction": ENVELOPE}),
        json!({"status": "ERROR", "errorResultXdr": ERROR_XDR}),
    )
    .up_to_n_times(1)
    .expect(1)
    .mount(&server)
    .await;
    assert_eq!(
        client(&server).submit(ENVELOPE, PASSPHRASE, policy()).await,
        Err(SentinelError::TransactionRejected {
            tx_hash: HASH.into(),
            code: Some(TransactionResultCode::TxTooEarly)
        })
    );

    rpc(
        "sendTransaction",
        json!({"transaction": ENVELOPE}),
        json!({"status": "PENDING"}),
    )
    .expect(1)
    .mount(&server)
    .await;
    rpc(
        "getTransaction",
        json!({"hash": HASH}),
        json!({"status": "NOT_FOUND"}),
    )
    .up_to_n_times(1)
    .expect(1)
    .mount(&server)
    .await;
    rpc(
        "getTransaction",
        json!({"hash": HASH}),
        json!({"status": "FAILED", "resultXdr": ERROR_XDR}),
    )
    .expect(1)
    .mount(&server)
    .await;
    assert_eq!(
        client(&server)
            .submit(ENVELOPE, PASSPHRASE, policy())
            .await
            .unwrap(),
        SubmissionOutcome::Failed {
            tx_hash: HASH.into(),
            result_xdr: ERROR_XDR.into()
        }
    );
    assert_eq!(server.received_requests().await.unwrap().len(), 4);
}

#[tokio::test]
async fn submitter_times_out_after_acceptance_and_repeated_misses() {
    let server = MockServer::start().await;
    rpc(
        "sendTransaction",
        json!({"transaction": ENVELOPE}),
        json!({"status": "PENDING"}),
    )
    .expect(1)
    .mount(&server)
    .await;
    rpc(
        "getTransaction",
        json!({"hash": HASH}),
        json!({"status": "NOT_FOUND"}),
    )
    .expect(1..)
    .mount(&server)
    .await;
    assert_eq!(
        client(&server)
            .submit(
                ENVELOPE,
                PASSPHRASE,
                SubmissionPolicy {
                    poll_timeout: Duration::from_millis(150),
                    ..policy()
                }
            )
            .await
            .unwrap(),
        SubmissionOutcome::TimedOut {
            tx_hash: HASH.into()
        }
    );
}
