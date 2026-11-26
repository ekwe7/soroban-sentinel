//! Public use case for fetching authoritative network limits.

use stellar_xdr::curr::{ConfigSettingEntry, LedgerEntry, LedgerEntryData, ReadXdr};

use crate::adapters::{
    derive_contract_data_entry_size_config_key, derive_state_archival_config_key, safe_xdr_limits,
};
use crate::domain::NetworkLimits;
use crate::error::{SentinelError, SentinelResult};
use crate::ports::LedgerReader;

/// Fetches state archival and contract-data entry limits from the network.
pub async fn fetch_network_limits<R: LedgerReader>(reader: &R) -> SentinelResult<NetworkLimits> {
    let archival_key = derive_state_archival_config_key()?;
    let size_key = derive_contract_data_entry_size_config_key()?;
    let latest = reader.get_latest_ledger().await?;
    let entries = reader.get_ledger_entries(&[archival_key, size_key]).await?;

    let mut archival = None;
    let mut max_entry_bytes = None;

    for entry in &entries {
        let ledger_entry = LedgerEntry::from_xdr_base64(&entry.entry_xdr_base64, safe_xdr_limits())
            .map_err(|error| SentinelError::MalformedNetworkConfig(error.to_string()))?;
        match ledger_entry.data {
            LedgerEntryData::ConfigSetting(ConfigSettingEntry::StateArchival(settings)) => {
                archival = Some(settings);
            }
            LedgerEntryData::ConfigSetting(ConfigSettingEntry::ContractDataEntrySizeBytes(
                bytes,
            )) => {
                max_entry_bytes = Some(bytes);
            }
            LedgerEntryData::ConfigSetting(_) => {
                return Err(SentinelError::UnexpectedNetworkConfigVariant(
                    "config entry did not match either requested setting".to_string(),
                ))
            }
            _ => {
                return Err(SentinelError::UnexpectedNetworkConfigVariant(
                    "ledger entry is not a ConfigSetting".to_string(),
                ))
            }
        }
    }

    let archival =
        archival.ok_or_else(|| SentinelError::MissingNetworkConfig("StateArchival".to_string()))?;
    let max_entry_bytes = max_entry_bytes.ok_or_else(|| {
        SentinelError::MissingNetworkConfig("ContractDataEntrySizeBytes".to_string())
    })?;

    Ok(NetworkLimits {
        max_entry_ttl: archival.max_entry_ttl,
        min_persistent_ttl: archival.min_persistent_ttl,
        min_temporary_ttl: archival.min_temporary_ttl,
        max_contract_data_entry_bytes: max_entry_bytes,
        protocol_version: latest.protocol_version,
    })
}
