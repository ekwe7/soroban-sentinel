use stellar_xdr::curr::{
    ExtensionPoint, LedgerFootprint, LedgerKey, Limits, Memo, MuxedAccount, Operation,
    OperationBody, Preconditions, SequenceNumber, SorobanResources, SorobanTransactionData,
    Transaction, TransactionEnvelope, TransactionExt, TransactionV1Envelope, Uint256, VecM,
    WriteXdr,
};

use crate::adapters::MAX_KEYS_PER_REQUEST;
use crate::error::{SentinelError, SentinelResult};

/// Builds an unsigned ExtendFootprintTTL transaction for read-only simulation.
pub fn build_extend_footprint_ttl_envelope(
    keys: &[LedgerKey],
    extend_to: u32,
    source_account: [u8; 32],
) -> SentinelResult<TransactionEnvelope> {
    if keys.is_empty() {
        return Err(SentinelError::InvalidXdrKey(
            "at least one ledger key is required for simulation".to_owned(),
        ));
    }
    if keys.len() > MAX_KEYS_PER_REQUEST {
        return Err(SentinelError::InvalidXdrKey(format!(
            "simulation footprint cannot contain more than {MAX_KEYS_PER_REQUEST} keys"
        )));
    }
    if extend_to == 0 {
        return Err(SentinelError::InvalidXdrKey(
            "extend_to must be greater than 0".to_owned(),
        ));
    }
    let footprint = LedgerFootprint {
        read_only: VecM::try_from(keys.to_vec())
            .map_err(|error| SentinelError::InvalidXdrKey(error.to_string()))?,
        read_write: VecM::default(),
    };
    let transaction_data = SorobanTransactionData {
        ext: ExtensionPoint::V0,
        resources: SorobanResources {
            footprint,
            instructions: 0,
            read_bytes: 0,
            write_bytes: 0,
        },
        resource_fee: 0,
    };
    let operation = Operation {
        source_account: None,
        body: OperationBody::ExtendFootprintTtl(stellar_xdr::curr::ExtendFootprintTtlOp {
            ext: ExtensionPoint::V0,
            extend_to,
        }),
    };
    let operations = VecM::try_from(vec![operation])
        .map_err(|error| SentinelError::InvalidXdrKey(error.to_string()))?;
    Ok(TransactionEnvelope::Tx(TransactionV1Envelope {
        tx: Transaction {
            source_account: MuxedAccount::Ed25519(Uint256(source_account)),
            fee: 0,
            seq_num: SequenceNumber(0),
            cond: Preconditions::None,
            memo: Memo::None,
            operations,
            ext: TransactionExt::V1(transaction_data),
        },
        signatures: VecM::default(),
    }))
}

pub fn envelope_to_xdr_base64(envelope: &TransactionEnvelope) -> SentinelResult<String> {
    envelope
        .to_xdr_base64(Limits::none())
        .map_err(|error| SentinelError::SerializationError(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use stellar_xdr::curr::{
        ContractDataDurability, Hash, LedgerKeyContractData, OperationBody, ReadXdr, ScAddress,
        ScVal,
    };

    fn key(seed: u8) -> LedgerKey {
        LedgerKey::ContractData(LedgerKeyContractData {
            contract: ScAddress::Contract(Hash([seed; 32])),
            key: ScVal::U32(seed as u32),
            durability: ContractDataDurability::Persistent,
        })
    }

    #[test]
    fn envelope_round_trips_extend_ttl_with_read_only_footprint() {
        let keys = vec![key(1), key(2)];
        let envelope = build_extend_footprint_ttl_envelope(&keys, 9_999, [7; 32]).unwrap();
        let xdr = envelope_to_xdr_base64(&envelope).unwrap();
        let decoded = TransactionEnvelope::from_xdr_base64(&xdr, Limits::none()).unwrap();
        let TransactionEnvelope::Tx(decoded) = decoded else {
            panic!("expected v1 envelope")
        };
        assert_eq!(decoded.tx.operations.len(), 1);
        assert!(matches!(
            decoded.tx.operations[0].body,
            OperationBody::ExtendFootprintTtl(stellar_xdr::curr::ExtendFootprintTtlOp {
                extend_to: 9_999,
                ..
            })
        ));
        let TransactionExt::V1(data) = decoded.tx.ext else {
            panic!("expected soroban data")
        };
        assert_eq!(
            data.resources
                .footprint
                .read_only
                .iter()
                .cloned()
                .collect::<Vec<_>>(),
            keys
        );
        assert!(data.resources.footprint.read_write.is_empty());
    }

    #[test]
    fn envelope_rejects_empty_and_oversized_footprints() {
        assert!(build_extend_footprint_ttl_envelope(&[], 1, [0; 32]).is_err());
        let keys = (0..=MAX_KEYS_PER_REQUEST as u8)
            .map(key)
            .collect::<Vec<_>>();
        assert!(build_extend_footprint_ttl_envelope(&keys, 1, [0; 32]).is_err());
    }
}
