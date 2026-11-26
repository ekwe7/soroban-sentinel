use std::str::FromStr;

use stellar_strkey::Contract;
use stellar_xdr::curr::{
    ContractDataDurability, Hash, LedgerKey, LedgerKeyContractData, Limits, ScAddress, ScSymbol,
    ScVal, WriteXdr,
};

/// Returns the inclusive XDR lower bound and exclusive upper bound around the
/// canonical persistent `LedgerKey` for `symbol_prefix`.
///
/// `contract_id` must be a valid contract StrKey and `symbol_prefix` must be a
/// valid Soroban symbol. Since XDR stores a symbol's length before its bytes,
/// these bounds cover the encoded base symbol, not every longer textual symbol
/// that starts with it. An empty upper bound represents an unbounded range
/// when no finite binary successor exists.
pub fn derive_key_prefix_range(contract_id: &str, symbol_prefix: &str) -> (Vec<u8>, Vec<u8>) {
    let contract = Contract::from_str(contract_id)
        .expect("contract_id must be a valid Stellar contract StrKey");
    let symbol = ScSymbol(
        symbol_prefix
            .try_into()
            .expect("symbol_prefix must be a valid Soroban symbol"),
    );
    let key = LedgerKey::ContractData(LedgerKeyContractData {
        contract: ScAddress::Contract(Hash(contract.0)),
        key: ScVal::Symbol(symbol),
        durability: ContractDataDurability::Persistent,
    });
    let lower = key
        .to_xdr(Limits::none())
        .expect("serializing a valid LedgerKey cannot fail");
    let upper = binary_successor(&lower).unwrap_or_default();

    (lower, upper)
}

fn binary_successor(bytes: &[u8]) -> Option<Vec<u8>> {
    let mut successor = bytes.to_vec();
    let index = successor.iter().rposition(|byte| *byte != u8::MAX)?;
    successor[index] += 1;
    successor.truncate(index + 1);
    Some(successor)
}
