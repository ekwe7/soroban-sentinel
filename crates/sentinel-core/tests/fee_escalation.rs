use sentinel_core::adapters::{
    build_extend_footprint_ttl_envelope, build_fee_bump_envelope, fee_bump_envelope_to_xdr_base64,
    SorobanRpcClient,
};
use sentinel_core::domain::{recommend_inclusion_fee, FeeUrgency};
use stellar_xdr::curr::{
    ContractDataDurability, FeeBumpTransactionInnerTx, Hash, LedgerKey, LedgerKeyContractData,
    Limits, ReadXdr, ScAddress, ScVal, TransactionEnvelope, Uint256,
};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn fee_stats_drive_urgency_percentiles_and_respect_the_cap() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "sorobanInclusionFee": {
                    "p50": "100",
                    "p80": "600",
                    "p90": "1200"
                }
            }
        })))
        .expect(1)
        .mount(&server)
        .await;

    let stats = SorobanRpcClient::new(server.uri())
        .get_fee_stats()
        .await
        .expect("feeStats should parse");
    let recommendation = recommend_inclusion_fee(stats, FeeUrgency::Critical, 1_000).unwrap();

    assert_eq!(recommendation.percentile, 90);
    assert_eq!(recommendation.fee_stroops, 1_000);
    assert!(recommendation.capped);
}

#[test]
fn replacement_fee_bump_xdr_preserves_inner_transaction_and_replaces_old_bump() {
    let key = LedgerKey::ContractData(LedgerKeyContractData {
        contract: ScAddress::Contract(Hash([4; 32])),
        key: ScVal::U32(1),
        durability: ContractDataDurability::Persistent,
    });
    let mut previous = build_extend_footprint_ttl_envelope(&[key], 9_999, [3; 32]).unwrap();
    let TransactionEnvelope::Tx(inner) = &mut previous else {
        panic!("builder should return a v1 envelope");
    };
    inner.tx.fee = 100;

    let first_bump = build_fee_bump_envelope(previous, [8; 32], 900).unwrap();
    let replacement = build_fee_bump_envelope(first_bump, [9; 32], 1_200).unwrap();
    let encoded = fee_bump_envelope_to_xdr_base64(&replacement).unwrap();
    let decoded = TransactionEnvelope::from_xdr_base64(&encoded, Limits::none()).unwrap();

    let TransactionEnvelope::TxFeeBump(bump) = decoded else {
        panic!("replacement should be a fee-bump envelope");
    };
    assert_eq!(bump.tx.fee, 1_200);
    assert_eq!(
        bump.tx.fee_source,
        stellar_xdr::curr::MuxedAccount::Ed25519(Uint256([9; 32]))
    );
    assert!(bump.signatures.is_empty());
    let FeeBumpTransactionInnerTx::Tx(inner) = bump.tx.inner_tx;
    assert_eq!(inner.tx.fee, 100);
    assert_eq!(inner.tx.operations.len(), 1);
}

#[test]
fn fee_bump_requires_a_strictly_higher_replacement_fee() {
    let key = LedgerKey::ContractData(LedgerKeyContractData {
        contract: ScAddress::Contract(Hash([4; 32])),
        key: ScVal::U32(1),
        durability: ContractDataDurability::Persistent,
    });
    let previous = build_extend_footprint_ttl_envelope(&[key], 9_999, [3; 32]).unwrap();
    let error = build_fee_bump_envelope(previous, [8; 32], 0).unwrap_err();
    assert!(error.to_string().contains("must exceed previous fee"));
}
