use sentinel_core::adapters::{
    build_extend_footprint_ttl_envelope, envelope_to_xdr_base64, inspect_transaction_envelope,
    unwrap_fee_bump_envelope,
};
use stellar_xdr::curr::*;

fn inner() -> TransactionV1Envelope {
    let TransactionEnvelope::Tx(mut tx) = build_extend_footprint_ttl_envelope(
        &[LedgerKey::ContractCode(LedgerKeyContractCode {
            hash: Hash([2; 32]),
        })],
        100,
        [1; 32],
    )
    .unwrap() else {
        unreachable!()
    };
    tx.tx.seq_num = SequenceNumber(123);
    tx.tx.fee = 100;
    tx
}

#[test]
fn ordinary_envelope_passthrough_preserves_operations_and_footprint() {
    let tx = inner();
    let encoded = envelope_to_xdr_base64(&TransactionEnvelope::Tx(tx.clone())).unwrap();
    assert_eq!(unwrap_fee_bump_envelope(&encoded).unwrap(), tx.tx);
    let info = inspect_transaction_envelope(&encoded).unwrap();
    assert_eq!(info.fee_sponsor, None);
    assert_eq!(info.maximum_fee_bid, 100);
    assert_eq!(info.inner_sequence, 123);
}

#[test]
fn fee_bump_preserves_inner_transaction_and_muxed_sponsor() {
    let tx = inner();
    let sponsor = MuxedAccount::MuxedEd25519(MuxedAccountMed25519 {
        id: 77,
        ed25519: Uint256([9; 32]),
    });
    let outer = TransactionEnvelope::TxFeeBump(FeeBumpTransactionEnvelope {
        tx: FeeBumpTransaction {
            fee_source: sponsor.clone(),
            fee: 10000,
            inner_tx: FeeBumpTransactionInnerTx::Tx(tx.clone()),
            ext: FeeBumpTransactionExt::V0,
        },
        signatures: VecM::default(),
    });
    let encoded = envelope_to_xdr_base64(&outer).unwrap();
    let info = inspect_transaction_envelope(&encoded).unwrap();
    assert_eq!(info.transaction, tx.tx);
    assert_eq!(info.fee_sponsor, Some(sponsor));
    assert_eq!(info.maximum_fee_bid, 10000);
    assert_eq!(info.inner_sequence, 123);
    assert_eq!(unwrap_fee_bump_envelope(&encoded).unwrap(), tx.tx);
}

#[test]
fn malformed_and_trailing_data_are_rejected_descriptively() {
    for text in ["", "not-base64!", "AAAAAg==", "AAAAAA=="] {
        let error = unwrap_fee_bump_envelope(text).unwrap_err().to_string();
        assert!(
            error.contains("cannot decode base64 transaction envelope"),
            "{error}"
        );
    }
    let valid = envelope_to_xdr_base64(&TransactionEnvelope::Tx(inner())).unwrap();
    assert!(unwrap_fee_bump_envelope(&(valid + "AAAA")).is_err());
}
