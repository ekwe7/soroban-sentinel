use serde::{Deserialize, Serialize};
use stellar_xdr::curr::{LedgerKey, ReadXdr};

use crate::adapters::{
    build_extend_footprint_ttl_envelope, derive_contract_code_key, derive_contract_instance_key,
    derive_persistent_key_from_scval, derive_persistent_symbol_key, derive_persistent_xdr_key,
    envelope_to_xdr_base64, extract_wasm_hash_from_instance_entry, parse_contract_id,
    parse_key_expr,
};
use crate::error::SentinelResult;
use crate::ports::{LedgerReader, SimulationEngine};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ForecastRequest {
    pub contract_id: String,
    pub extend_to: u32,
    pub source_account: [u8; 32],
    pub persistent_symbols: Vec<String>,
    pub persistent_xdr_keys: Vec<String>,
    pub persistent_key_exprs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForecastReport {
    pub contract_id: String,
    pub extend_to: u32,
    pub latest_ledger: u32,
    pub min_resource_fee_stroops: u64,
    pub covered_entries: Vec<String>,
}

pub struct ForecastExtensionUseCase<R> {
    reader: R,
}

impl<R> ForecastExtensionUseCase<R>
where
    R: LedgerReader + SimulationEngine,
{
    pub fn new(reader: R) -> Self {
        Self { reader }
    }

    pub async fn execute(&self, request: ForecastRequest) -> SentinelResult<ForecastReport> {
        let contract_id = parse_contract_id(&request.contract_id)?;
        let latest = self.reader.get_latest_ledger().await?;
        let instance_key = derive_contract_instance_key(&contract_id)?;
        let mut keys =
            vec![
                LedgerKey::from_xdr_base64(&instance_key, crate::adapters::safe_xdr_limits())
                    .map_err(|error| {
                        crate::error::SentinelError::InvalidXdrKey(error.to_string())
                    })?,
            ];
        let mut covered_entries = vec!["instance".to_owned()];
        let mut persistent_descriptors = Vec::new();
        for symbol in &request.persistent_symbols {
            persistent_descriptors.push((
                format!("persistent:{symbol}"),
                derive_persistent_symbol_key(&contract_id, symbol)?,
            ));
        }
        for key_xdr in &request.persistent_xdr_keys {
            persistent_descriptors.push((
                format!("persistent_xdr:{key_xdr}"),
                derive_persistent_xdr_key(&contract_id, key_xdr)?,
            ));
        }
        for expression in &request.persistent_key_exprs {
            let scval = parse_key_expr(expression)?;
            persistent_descriptors.push((
                format!("persistent:{expression}"),
                derive_persistent_key_from_scval(&contract_id, scval)?,
            ));
        }
        for (name, key_xdr) in &persistent_descriptors {
            keys.push(
                LedgerKey::from_xdr_base64(key_xdr, crate::adapters::safe_xdr_limits()).map_err(
                    |error| crate::error::SentinelError::InvalidXdrKey(error.to_string()),
                )?,
            );
            covered_entries.push(name.clone());
        }
        let query_key_xdrs = std::iter::once(instance_key.clone())
            .chain(
                persistent_descriptors
                    .iter()
                    .map(|(_, key_xdr)| key_xdr.clone()),
            )
            .collect::<Vec<_>>();
        let raw_entries = self.reader.get_ledger_entries(&query_key_xdrs).await?;
        if let Some(instance) = raw_entries
            .iter()
            .find(|entry| entry.key_xdr_base64 == instance_key)
        {
            if let Some(wasm_hash) =
                extract_wasm_hash_from_instance_entry(&instance.entry_xdr_base64)?
            {
                let code_key_xdr = derive_contract_code_key(&wasm_hash)?;
                let code_key =
                    LedgerKey::from_xdr_base64(&code_key_xdr, crate::adapters::safe_xdr_limits())
                        .map_err(|error| {
                        crate::error::SentinelError::InvalidXdrKey(error.to_string())
                    })?;
                keys.push(code_key);
                covered_entries.push("code_wasm".to_owned());
            }
        }
        if request.extend_to == 0 {
            return Err(crate::error::SentinelError::InvalidXdrKey(
                "extend_to must be greater than 0".to_owned(),
            ));
        }
        let envelope =
            build_extend_footprint_ttl_envelope(&keys, request.extend_to, request.source_account)?;
        let transaction_xdr = envelope_to_xdr_base64(&envelope)?;
        let estimate =
            SimulationEngine::simulate_transaction(&self.reader, &transaction_xdr, latest.sequence)
                .await?;
        Ok(ForecastReport {
            contract_id: request.contract_id,
            extend_to: request.extend_to,
            latest_ledger: estimate.latest_ledger,
            min_resource_fee_stroops: estimate.min_resource_fee_stroops,
            covered_entries,
        })
    }
}
