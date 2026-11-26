use stellar_xdr::curr::{
    ContractCodeEntry, ContractDataDurability, Hash, LedgerKey, LedgerKeyContractData, ScAddress,
};

use crate::{adapters::wasm_spec::storage_key_candidates, parse_contract_id, SentinelResult};

/// Derive persistent and temporary targets for each inferred storage key.
/// The results are candidates. A scan must confirm which entries exist.
pub fn discover_storage_keys(
    contract_id: &str,
    code: &ContractCodeEntry,
) -> SentinelResult<Vec<LedgerKey>> {
    let contract = ScAddress::Contract(Hash(parse_contract_id(contract_id)?));
    let candidates = storage_key_candidates(code)?;
    Ok(candidates
        .into_iter()
        .flat_map(|key| {
            [
                ContractDataDurability::Persistent,
                ContractDataDurability::Temporary,
            ]
            .map(|durability| {
                LedgerKey::ContractData(LedgerKeyContractData {
                    contract: contract.clone(),
                    key: key.clone(),
                    durability,
                })
            })
        })
        .collect())
}
