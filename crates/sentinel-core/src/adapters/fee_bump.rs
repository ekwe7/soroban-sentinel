//! Envelope inspection without signing, authorization checks, or submission.

use super::{safe_xdr_limits, XDR_MAX_BYTE_LENGTH};
use crate::SentinelError;
use stellar_xdr::curr::{
    FeeBumpTransactionInnerTx, MuxedAccount, Preconditions, ReadXdr, Transaction,
    TransactionEnvelope, TransactionExt,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InspectedEnvelope {
    pub transaction: Transaction,
    /// Exact fee-source account, retaining an optional muxed account id.
    pub fee_sponsor: Option<MuxedAccount>,
    pub maximum_fee_bid: i64,
    pub inner_sequence: i64,
}

pub fn inspect_transaction_envelope(
    envelope_xdr: &str,
) -> Result<InspectedEnvelope, SentinelError> {
    if envelope_xdr.len() as u64 > (XDR_MAX_BYTE_LENGTH as u64).div_ceil(3) * 4 {
        return Err(SentinelError::InvalidTransactionEnvelope(
            "base64 envelope exceeds the XDR size limit".into(),
        ));
    }
    let envelope =
        TransactionEnvelope::from_xdr_base64(envelope_xdr, safe_xdr_limits()).map_err(|error| {
            SentinelError::InvalidTransactionEnvelope(format!(
                "cannot decode base64 transaction envelope: {error}"
            ))
        })?;
    let (transaction, fee_sponsor, maximum_fee_bid) = match envelope {
        TransactionEnvelope::Tx(value) => {
            let fee = i64::from(value.tx.fee);
            (value.tx, None, fee)
        }
        TransactionEnvelope::TxFeeBump(value) => {
            let FeeBumpTransactionInnerTx::Tx(inner) = value.tx.inner_tx;
            (inner.tx, Some(value.tx.fee_source), value.tx.fee)
        }
        TransactionEnvelope::TxV0(value) => {
            let tx = value.tx;
            let fee = i64::from(tx.fee);
            (
                Transaction {
                    source_account: MuxedAccount::Ed25519(tx.source_account_ed25519),
                    fee: tx.fee,
                    seq_num: tx.seq_num,
                    cond: tx
                        .time_bounds
                        .map_or(Preconditions::None, Preconditions::Time),
                    memo: tx.memo,
                    operations: tx.operations,
                    ext: TransactionExt::V0,
                },
                None,
                fee,
            )
        }
    };
    Ok(InspectedEnvelope {
        inner_sequence: transaction.seq_num.0,
        transaction,
        fee_sponsor,
        maximum_fee_bid,
    })
}

pub fn unwrap_fee_bump_envelope(envelope_xdr: &str) -> Result<Transaction, SentinelError> {
    Ok(inspect_transaction_envelope(envelope_xdr)?.transaction)
}
