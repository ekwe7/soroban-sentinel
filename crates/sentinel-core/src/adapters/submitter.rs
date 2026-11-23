use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use stellar_xdr::curr::{
    Hash, MuxedAccount, ReadXdr, Transaction, TransactionEnvelope, TransactionExt,
    TransactionResult, TransactionSignaturePayload, TransactionSignaturePayloadTaggedTransaction,
    WriteXdr,
};

use super::{safe_xdr_limits, SorobanRpcClient};
use crate::{
    SentinelError, SentinelResult, SubmissionOutcome, SubmissionPolicy, TransactionSubmitter,
};

#[derive(Deserialize)]
#[serde(tag = "status", rename_all = "SCREAMING_SNAKE_CASE")]
enum SendResult {
    Pending,
    Duplicate,
    TryAgainLater,
    Error {
        #[serde(rename = "errorResultXdr")]
        error_result_xdr: Option<String>,
    },
}

#[derive(Deserialize)]
#[serde(tag = "status", rename_all = "SCREAMING_SNAKE_CASE")]
enum LookupResult {
    Success {
        ledger: u32,
    },
    Failed {
        #[serde(rename = "resultXdr")]
        result_xdr: String,
    },
    NotFound,
}

pub(super) fn transaction_hash(envelope: &str, passphrase: &str) -> SentinelResult<String> {
    use TransactionSignaturePayloadTaggedTransaction::{Tx, TxFeeBump};

    let envelope = TransactionEnvelope::from_xdr_base64(envelope, safe_xdr_limits())
        .map_err(|error| SentinelError::InvalidTransactionEnvelope(error.to_string()))?;
    let tagged_transaction = match envelope {
        TransactionEnvelope::Tx(value) => Tx(value.tx),
        TransactionEnvelope::TxFeeBump(value) => TxFeeBump(value.tx),
        TransactionEnvelope::TxV0(value) => {
            let tx = value.tx;
            Tx(Transaction {
                source_account: MuxedAccount::Ed25519(tx.source_account_ed25519),
                fee: tx.fee,
                seq_num: tx.seq_num,
                cond: tx.time_bounds.map_or(
                    stellar_xdr::curr::Preconditions::None,
                    stellar_xdr::curr::Preconditions::Time,
                ),
                memo: tx.memo,
                operations: tx.operations,
                ext: TransactionExt::V0,
            })
        }
    };
    let payload = TransactionSignaturePayload {
        network_id: Hash(Sha256::digest(passphrase.as_bytes()).into()),
        tagged_transaction,
    }
    .to_xdr(safe_xdr_limits())
    .map_err(|error| SentinelError::InvalidTransactionEnvelope(error.to_string()))?;
    Ok(format!("{:x}", Sha256::digest(payload)))
}

#[async_trait]
impl TransactionSubmitter for SorobanRpcClient {
    async fn submit(
        &self,
        signed_envelope: &str,
        network_passphrase: &str,
        policy: SubmissionPolicy,
    ) -> SentinelResult<SubmissionOutcome> {
        let tx_hash = transaction_hash(signed_envelope, network_passphrase)?;
        let send = tokio::time::timeout(policy.send_timeout, async {
            for attempt in 1..=self.retry_policy().max_retries.max(1) {
                match self
                    .send_rpc_request("sendTransaction", json!({"transaction": signed_envelope}))
                    .await?
                {
                    SendResult::Pending | SendResult::Duplicate => return Ok(()),
                    SendResult::TryAgainLater if attempt < self.retry_policy().max_retries => {
                        tokio::time::sleep(self.retry_policy().next_delay(attempt, None)).await;
                    }
                    SendResult::TryAgainLater => break,
                    SendResult::Error { error_result_xdr } => {
                        let code = error_result_xdr
                            .and_then(|xdr| {
                                TransactionResult::from_xdr_base64(xdr, safe_xdr_limits()).ok()
                            })
                            .map(|result| result.result.discriminant());
                        return Err(SentinelError::TransactionRejected {
                            tx_hash: tx_hash.clone(),
                            code,
                        });
                    }
                }
            }
            Err(SentinelError::TransactionRetryExhausted(tx_hash.clone()))
        })
        .await;
        match send {
            Ok(Ok(())) => {}
            Ok(Err(SentinelError::RpcConnectionTimeout(_))) | Err(_) => {
                // A timed-out submission may have reached the node. Check the
                // locally derived hash instead of sending it again.
            }
            Ok(Err(error)) => return Err(error),
        }

        let polling = async {
            loop {
                match self
                    .send_rpc_request("getTransaction", json!({"hash": tx_hash}))
                    .await
                {
                    Ok(LookupResult::Success { ledger: 0 }) => {
                        return Err(SentinelError::SerializationError(
                            "Confirmed transaction has no ledger sequence".into(),
                        ));
                    }
                    Ok(LookupResult::Success { ledger }) => {
                        return Ok(SubmissionOutcome::Confirmed {
                            ledger,
                            tx_hash: tx_hash.clone(),
                        })
                    }
                    Ok(LookupResult::Failed { result_xdr }) => {
                        return Ok(SubmissionOutcome::Failed {
                            tx_hash: tx_hash.clone(),
                            result_xdr,
                        })
                    }
                    Ok(LookupResult::NotFound) | Err(SentinelError::RpcConnectionTimeout(_)) => {}
                    Err(error) => return Err(error),
                }
                tokio::time::sleep(policy.poll_interval.max(Duration::from_millis(1))).await;
            }
        };
        tokio::time::timeout(policy.poll_timeout, polling)
            .await
            .unwrap_or(Ok(SubmissionOutcome::TimedOut { tx_hash }))
    }
}
