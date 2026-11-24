//! CAP-0015 fee-bump transaction envelope construction.

use stellar_xdr::curr::{
    FeeBumpTransaction, FeeBumpTransactionEnvelope, FeeBumpTransactionExt,
    FeeBumpTransactionInnerTx, Limits, MuxedAccount, TransactionEnvelope, Uint256, VecM, WriteXdr,
};

use crate::error::{SentinelError, SentinelResult};

/// Wraps or replaces a fee-bump envelope while preserving the original signed inner transaction.
pub fn build_fee_bump_envelope(
    previous_envelope: TransactionEnvelope,
    fee_source: [u8; 32],
    new_fee_stroops: u64,
) -> SentinelResult<TransactionEnvelope> {
    let (inner, previous_fee) = match previous_envelope {
        TransactionEnvelope::Tx(inner) => {
            let previous_fee = u64::from(inner.tx.fee);
            (inner, previous_fee)
        }
        TransactionEnvelope::TxFeeBump(previous) => {
            let previous_fee = nonnegative_fee(previous.tx.fee)?;
            let FeeBumpTransactionInnerTx::Tx(inner) = previous.tx.inner_tx;
            (inner, previous_fee)
        }
        TransactionEnvelope::TxV0(_) => {
            return Err(SentinelError::InvalidTransactionEnvelope(
                "fee bumps require a v1 transaction envelope".to_string(),
            ))
        }
    };

    if new_fee_stroops <= previous_fee {
        return Err(SentinelError::InvalidTransactionEnvelope(format!(
            "replacement fee {new_fee_stroops} must exceed previous fee {previous_fee}"
        )));
    }
    let fee = i64::try_from(new_fee_stroops).map_err(|_| {
        SentinelError::InvalidTransactionEnvelope(
            "fee bump amount exceeds the XDR int64 range".to_string(),
        )
    })?;

    Ok(TransactionEnvelope::TxFeeBump(FeeBumpTransactionEnvelope {
        tx: FeeBumpTransaction {
            fee_source: MuxedAccount::Ed25519(Uint256(fee_source)),
            fee,
            inner_tx: FeeBumpTransactionInnerTx::Tx(inner),
            ext: FeeBumpTransactionExt::V0,
        },
        signatures: VecM::default(),
    }))
}

/// Serializes a transaction envelope to base64 XDR.
pub fn fee_bump_envelope_to_xdr_base64(envelope: &TransactionEnvelope) -> SentinelResult<String> {
    envelope
        .to_xdr_base64(Limits::none())
        .map_err(|error| SentinelError::SerializationError(error.to_string()))
}

fn nonnegative_fee(fee: i64) -> SentinelResult<u64> {
    u64::try_from(fee).map_err(|_| {
        SentinelError::InvalidTransactionEnvelope(
            "previous transaction has a negative fee".to_string(),
        )
    })
}
