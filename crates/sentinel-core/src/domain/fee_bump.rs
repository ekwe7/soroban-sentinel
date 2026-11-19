//! Fee-stat based inclusion recommendations and fee-bump envelope construction.

use serde::{Deserialize, Deserializer};
use serde_json::Value;
use stellar_xdr::curr::{
    FeeBumpTransaction, FeeBumpTransactionEnvelope, FeeBumpTransactionExt,
    FeeBumpTransactionInnerTx, Limits, MuxedAccount, ReadXdr, TransactionEnvelope, VecM, WriteXdr,
};

use crate::adapters::safe_xdr_limits;
use crate::error::{SentinelError, SentinelResult};

/// Soroban fee percentile observations returned by the RPC `getFeeStats` method.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeePercentiles {
    #[serde(default, deserialize_with = "deserialize_optional_fee")]
    pub p10: Option<i64>,
    #[serde(default, deserialize_with = "deserialize_optional_fee")]
    pub p20: Option<i64>,
    #[serde(default, deserialize_with = "deserialize_optional_fee")]
    pub p30: Option<i64>,
    #[serde(default, deserialize_with = "deserialize_optional_fee")]
    pub p40: Option<i64>,
    #[serde(default, deserialize_with = "deserialize_optional_fee")]
    pub p50: Option<i64>,
    #[serde(default, deserialize_with = "deserialize_optional_fee")]
    pub p60: Option<i64>,
    #[serde(default, deserialize_with = "deserialize_optional_fee")]
    pub p70: Option<i64>,
    #[serde(default, deserialize_with = "deserialize_optional_fee")]
    pub p80: Option<i64>,
    #[serde(default, deserialize_with = "deserialize_optional_fee")]
    pub p90: Option<i64>,
    #[serde(default, deserialize_with = "deserialize_optional_fee")]
    pub p95: Option<i64>,
    #[serde(default, deserialize_with = "deserialize_optional_fee")]
    pub p99: Option<i64>,
}

/// Fee information returned by Soroban RPC `getFeeStats`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeeStats {
    pub soroban_inclusion_fee: FeePercentiles,
}

/// Relative urgency used to choose a network fee percentile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeeUrgency {
    /// Use the median observed Soroban inclusion fee.
    Low,
    /// Use the 70th percentile.
    Normal,
    /// Use the 80th percentile.
    High,
    /// Use the 90th percentile for critically expiring entries.
    Critical,
}

impl FeeUrgency {
    /// Percentile selected for this urgency.
    pub const fn percentile(self) -> u8 {
        match self {
            Self::Low => 50,
            Self::Normal => 70,
            Self::High => 80,
            Self::Critical => 90,
        }
    }

    fn fee(self, percentiles: &FeePercentiles) -> Option<i64> {
        match self {
            Self::Low => percentiles.p50,
            Self::Normal => percentiles.p70,
            Self::High => percentiles.p80,
            Self::Critical => percentiles.p90,
        }
    }
}

/// Recommended aggregate fee in stroops, along with the percentile used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FeeRecommendation {
    pub percentile: u8,
    /// Soroban inclusion fee per operation in stroops.
    pub fee_per_operation: i64,
    /// Aggregate fee bid in stroops for all operations, capped by the caller's safety limit.
    pub total_fee: i64,
    /// True when `total_fee` was limited by `max_total_fee`.
    pub capped: bool,
}

/// Selects an inclusion-fee percentile, scales it for the operation count, and caps spending.
pub fn recommend_inclusion_fee(
    stats: &FeeStats,
    urgency: FeeUrgency,
    operation_count: u32,
    max_total_fee: i64,
) -> SentinelResult<FeeRecommendation> {
    if operation_count == 0 {
        return Err(SentinelError::InvalidFeeBump(
            "Operation count must be greater than zero".to_string(),
        ));
    }
    if max_total_fee <= 0 {
        return Err(SentinelError::InvalidFeeBump(
            "Maximum total fee must be greater than zero".to_string(),
        ));
    }

    let fee_per_operation = urgency
        .fee(&stats.soroban_inclusion_fee)
        .filter(|fee| *fee > 0)
        .ok_or_else(|| {
            SentinelError::InvalidFeeBump(format!(
                "Missing or invalid p{} Soroban inclusion fee",
                urgency.percentile()
            ))
        })?;
    let uncapped_fee = fee_per_operation
        .checked_mul(i64::from(operation_count))
        .ok_or_else(|| {
            SentinelError::InvalidFeeBump("Calculated total fee overflowed".to_string())
        })?;
    let total_fee = uncapped_fee.min(max_total_fee);

    Ok(FeeRecommendation {
        percentile: urgency.percentile(),
        fee_per_operation,
        total_fee,
        capped: uncapped_fee > max_total_fee,
    })
}

/// Creates an unsigned replacement fee-bump envelope from a previous transaction envelope.
///
/// The inner transaction and its signatures are preserved. Any previous outer fee-bump
/// signatures are discarded because changing the fee invalidates them; the caller must sign
/// the returned envelope before submission.
pub fn build_replacement_fee_bump_envelope(
    previous_envelope_xdr_base64: &str,
    fee_source: MuxedAccount,
    new_total_fee: i64,
    max_total_fee: i64,
) -> SentinelResult<String> {
    if max_total_fee <= 0 || new_total_fee <= 0 {
        return Err(SentinelError::InvalidFeeBump(
            "Fee and maximum total fee must be greater than zero".to_string(),
        ));
    }
    if new_total_fee > max_total_fee {
        return Err(SentinelError::InvalidFeeBump(format!(
            "Requested fee {} exceeds maximum total fee {}",
            new_total_fee, max_total_fee
        )));
    }

    let previous =
        TransactionEnvelope::from_xdr_base64(previous_envelope_xdr_base64, safe_xdr_limits())
            .map_err(|error| {
                SentinelError::InvalidXdrKey(format!(
                    "Failed to decode transaction envelope: {}",
                    error
                ))
            })?;

    let (inner_tx, previous_fee) = match previous {
        TransactionEnvelope::Tx(inner) => {
            let fee = i64::from(inner.tx.fee);
            (inner, fee)
        }
        TransactionEnvelope::TxFeeBump(previous_bump) => {
            let inner = match previous_bump.tx.inner_tx {
                FeeBumpTransactionInnerTx::Tx(inner) => inner,
            };
            (inner, previous_bump.tx.fee)
        }
        TransactionEnvelope::TxV0(_) => {
            return Err(SentinelError::InvalidFeeBump(
                "Fee bumps require a version 1 transaction envelope".to_string(),
            ));
        }
    };

    if new_total_fee <= previous_fee {
        return Err(SentinelError::InvalidFeeBump(format!(
            "Replacement fee {} must exceed the previous fee {}",
            new_total_fee, previous_fee
        )));
    }

    let envelope = TransactionEnvelope::TxFeeBump(FeeBumpTransactionEnvelope {
        tx: FeeBumpTransaction {
            fee_source,
            fee: new_total_fee,
            inner_tx: FeeBumpTransactionInnerTx::Tx(inner_tx),
            ext: FeeBumpTransactionExt::V0,
        },
        signatures: VecM::default(),
    });

    envelope.to_xdr_base64(Limits::none()).map_err(|error| {
        SentinelError::SerializationError(format!(
            "Failed to encode replacement fee-bump envelope: {}",
            error
        ))
    })
}

fn deserialize_optional_fee<'de, D>(deserializer: D) -> Result<Option<i64>, D::Error>
where
    D: Deserializer<'de>,
{
    match Option::<Value>::deserialize(deserializer)? {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(number)) => number
            .as_i64()
            .map(Some)
            .ok_or_else(|| serde::de::Error::custom("fee percentile must be an integer")),
        Some(Value::String(value)) => value
            .parse::<i64>()
            .map(Some)
            .map_err(serde::de::Error::custom),
        Some(_) => Err(serde::de::Error::custom(
            "fee percentile must be an integer or decimal string",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use stellar_xdr::curr::{
        Memo, Preconditions, SequenceNumber, Transaction, TransactionExt, TransactionV1Envelope,
        Uint256,
    };

    fn sample_stats() -> FeeStats {
        FeeStats {
            soroban_inclusion_fee: FeePercentiles {
                p50: Some(500),
                p70: Some(700),
                p80: Some(800),
                p90: Some(900),
                ..FeePercentiles::default()
            },
        }
    }

    fn account(byte: u8) -> MuxedAccount {
        MuxedAccount::Ed25519(Uint256([byte; 32]))
    }

    fn sample_transaction_xdr() -> String {
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
    fn critical_urgency_uses_p90_and_applies_operation_count() {
        let recommendation =
            recommend_inclusion_fee(&sample_stats(), FeeUrgency::Critical, 2, 5_000).unwrap();
        assert_eq!(recommendation.percentile, 90);
        assert_eq!(recommendation.fee_per_operation, 900);
        assert_eq!(recommendation.total_fee, 1_800);
        assert!(!recommendation.capped);
    }

    #[test]
    fn recommendation_obeys_total_fee_cap() {
        let recommendation =
            recommend_inclusion_fee(&sample_stats(), FeeUrgency::Critical, 2, 1_000).unwrap();
        assert_eq!(recommendation.total_fee, 1_000);
        assert!(recommendation.capped);
    }

    #[test]
    fn replacement_is_a_fee_bump_with_same_inner_transaction() {
        let original_xdr = sample_transaction_xdr();
        let bumped_xdr =
            build_replacement_fee_bump_envelope(&original_xdr, account(2), 1_000, 2_000).unwrap();
        let original =
            TransactionEnvelope::from_xdr_base64(&original_xdr, safe_xdr_limits()).unwrap();
        let bumped = TransactionEnvelope::from_xdr_base64(&bumped_xdr, safe_xdr_limits()).unwrap();

        let TransactionEnvelope::Tx(original) = original else {
            panic!("expected original version 1 transaction");
        };
        let TransactionEnvelope::TxFeeBump(bumped) = bumped else {
            panic!("expected fee-bump transaction");
        };
        assert_eq!(bumped.tx.fee, 1_000);
        assert_eq!(bumped.tx.inner_tx, FeeBumpTransactionInnerTx::Tx(original));
        assert!(bumped.signatures.is_empty());
    }
}
