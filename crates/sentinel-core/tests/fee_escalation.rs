use sentinel_core::{
    build_replacement_fee_bump_envelope, recommend_inclusion_fee, FeePercentiles, FeeStats,
    FeeUrgency, SentinelError, SorobanRpcClient,
};
use stellar_xdr::curr::{
    FeeBumpTransactionInnerTx, Limits, Memo, MuxedAccount, Preconditions, ReadXdr, SequenceNumber,
    Transaction, TransactionEnvelope, TransactionExt, TransactionV1Envelope, Uint256, VecM,
    WriteXdr,
};
use wiremock::matchers::{body_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn fee_stats() -> FeeStats {
    FeeStats {
        soroban_inclusion_fee: FeePercentiles {
            p10: Some(100),
            p20: Some(200),
            p30: Some(300),
            p40: Some(400),
            p50: Some(500),
            p60: Some(600),
            p70: Some(700),
            p80: Some(800),
            p90: Some(900),
            p95: Some(950),
            p99: Some(990),
        },
    }
}

fn account(byte: u8) -> MuxedAccount {
    MuxedAccount::Ed25519(Uint256([byte; 32]))
}

fn transaction_envelope() -> String {
    let envelope = TransactionEnvelope::Tx(TransactionV1Envelope {
        tx: Transaction {
            source_account: account(1),
            fee: 100,
            seq_num: SequenceNumber(1),
            cond: Preconditions::None,
            memo: Memo::None,
            operations: VecM::default(),
            ext: TransactionExt::V0,
        },
        signatures: VecM::default(),
    });
    envelope.to_xdr_base64(Limits::none()).unwrap()
}

#[test]
fn urgency_selects_expected_percentile_and_scales_for_operations() {
    let stats = fee_stats();

    let normal = recommend_inclusion_fee(&stats, FeeUrgency::Normal, 2, 10_000).unwrap();
    assert_eq!(normal.percentile, 70);
    assert_eq!(normal.fee_per_operation, 700);
    assert_eq!(normal.total_fee, 1_400);
    assert!(!normal.capped);

    let critical = recommend_inclusion_fee(&stats, FeeUrgency::Critical, 3, 10_000).unwrap();
    assert_eq!(critical.percentile, 90);
    assert_eq!(critical.fee_per_operation, 900);
    assert_eq!(critical.total_fee, 2_700);
}

#[test]
fn fee_recommendation_caps_total_spending() {
    let recommendation =
        recommend_inclusion_fee(&fee_stats(), FeeUrgency::Critical, 3, 2_000).unwrap();

    assert_eq!(recommendation.total_fee, 2_000);
    assert!(recommendation.capped);
}

#[test]
fn fee_recommendation_rejects_missing_percentiles_invalid_counts_and_caps() {
    let missing = FeeStats {
        soroban_inclusion_fee: FeePercentiles::default(),
    };
    assert!(matches!(
        recommend_inclusion_fee(&missing, FeeUrgency::Critical, 1, 100),
        Err(SentinelError::InvalidFeeBump(_))
    ));
    assert!(recommend_inclusion_fee(&fee_stats(), FeeUrgency::Critical, 0, 100).is_err());
    assert!(recommend_inclusion_fee(&fee_stats(), FeeUrgency::Critical, 1, 0).is_err());
}

#[test]
fn rpc_percentiles_accept_integer_and_decimal_string_fees() {
    let parsed: FeeStats = serde_json::from_value(serde_json::json!({
        "sorobanInclusionFee": {
            "p50": 120,
            "p90": "900"
        }
    }))
    .unwrap();
    assert_eq!(parsed.soroban_inclusion_fee.p50, Some(120));
    assert_eq!(parsed.soroban_inclusion_fee.p90, Some(900));
}

#[test]
fn replacement_envelope_preserves_inner_transaction_and_increases_fee() {
    let original_xdr = transaction_envelope();
    let bump_xdr =
        build_replacement_fee_bump_envelope(&original_xdr, account(2), 1_000, 2_000).unwrap();
    let replacement_xdr =
        build_replacement_fee_bump_envelope(&bump_xdr, account(3), 1_500, 2_000).unwrap();

    let original = TransactionEnvelope::from_xdr_base64(&original_xdr, Limits::none()).unwrap();
    let replacement =
        TransactionEnvelope::from_xdr_base64(&replacement_xdr, Limits::none()).unwrap();
    let (TransactionEnvelope::Tx(original), TransactionEnvelope::TxFeeBump(replacement)) =
        (original, replacement)
    else {
        panic!("expected transaction and fee-bump envelopes");
    };

    assert_eq!(replacement.tx.fee, 1_500);
    assert_eq!(replacement.tx.fee_source, account(3));
    assert!(replacement.signatures.is_empty());
    assert_eq!(
        replacement.tx.inner_tx,
        FeeBumpTransactionInnerTx::Tx(original)
    );
}

#[test]
fn replacement_envelope_enforces_fee_growth_and_maximum() {
    let original_xdr = transaction_envelope();
    let bump_xdr =
        build_replacement_fee_bump_envelope(&original_xdr, account(2), 1_000, 2_000).unwrap();

    assert!(matches!(
        build_replacement_fee_bump_envelope(&bump_xdr, account(3), 1_000, 2_000),
        Err(SentinelError::InvalidFeeBump(_))
    ));
    assert!(matches!(
        build_replacement_fee_bump_envelope(&bump_xdr, account(3), 2_001, 2_000),
        Err(SentinelError::InvalidFeeBump(_))
    ));
}

#[tokio::test]
async fn soroban_rpc_queries_fee_stats() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/"))
        .and(body_json(serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "getFeeStats",
            "params": {}
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "sorobanInclusionFee": {
                    "min": "100",
                    "max": "1000",
                    "mode": "500",
                    "p50": "500",
                    "p70": "700",
                    "p80": "800",
                    "p90": "900",
                    "p95": "950",
                    "p99": "990"
                }
            }
        })))
        .expect(1)
        .mount(&server)
        .await;

    let stats = SorobanRpcClient::new(server.uri())
        .get_fee_stats()
        .await
        .unwrap();
    let recommendation = recommend_inclusion_fee(&stats, FeeUrgency::Critical, 2, 5_000).unwrap();
    assert_eq!(recommendation.total_fee, 1_800);
    server.verify().await;
}
