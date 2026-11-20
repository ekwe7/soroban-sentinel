//! Use case orchestrator for scanning Soroban smart contract storage TTL and expiration risk.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use stellar_xdr::curr::{ReadXdr, WriteXdr};

use crate::adapters::{
    derive_contract_code_key, derive_contract_data_entry_size_limit_key,
    derive_contract_instance_key, derive_persistent_key_from_scval, derive_persistent_symbol_key,
    derive_persistent_xdr_key, derive_temporary_symbol_key, derive_temporary_xdr_key,
    extract_contract_data_entry_size_limit, extract_instance_storage_stats,
    extract_wasm_hash_from_instance_entry, parse_contract_id, parse_key_expr, safe_xdr_limits,
};
use crate::domain::cold_storage::{
    recommend_archival, ArchivalPolicy, ArchivalRecommendation, ExtensionHistory,
};
use crate::domain::{
    calculate_remaining_ledgers, estimate_remaining_days, estimate_remaining_seconds_with_velocity,
    evaluate_risk, Durability, EntryType, InstanceSizeThresholds, RiskLevel, StorageEntry,
    ThresholdConfig, TARGET_LEDGER_CLOSE_SECONDS,
};
use crate::error::SentinelResult;
use crate::ports::reader::LedgerReader;

/// Request parameters for scanning a Soroban contract.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScanRequest {
    /// Contract address encoded as Stellar Strkey (e.g. `CA3D...`).
    pub contract_id_strkey: String,
    /// List of persistent storage symbol keys to monitor (e.g. `["COUNTER", "ADMIN"]`).
    pub persistent_symbols: Vec<String>,
    /// List of persistent storage keys encoded as base64 `ScVal` XDR strings.
    pub persistent_xdr_keys: Vec<String>,
    /// List of persistent storage keys encoded as typed expressions (e.g. `["vec[sym:Balance, addr:G...]"]`).
    pub persistent_key_exprs: Vec<String>,
    /// List of temporary storage symbol keys to monitor (e.g. `["NONCE"]`).
    #[serde(default)]
    pub temporary_symbols: Vec<String>,
    /// List of temporary storage keys encoded as base64 `ScVal` XDR strings.
    #[serde(default)]
    pub temporary_xdr_keys: Vec<String>,
    /// Threshold configuration for warning and critical expiry evaluation.
    pub threshold_config: ThresholdConfig,
    /// Instance-storage utilisation thresholds against the network entry-size limit.
    #[serde(default)]
    pub instance_size_thresholds: InstanceSizeThresholds,
    /// Target network descriptor (e.g. "testnet", "mainnet", etc.).
    pub network: Option<String>,
    /// Optional override for ledger close velocity in seconds per ledger.
    pub ledger_seconds_override: Option<f64>,
}

impl ScanRequest {
    /// Create a new `ScanRequest` with default thresholds and empty persistent keys.
    pub fn new(contract_id_strkey: impl Into<String>) -> Self {
        Self {
            contract_id_strkey: contract_id_strkey.into(),
            persistent_symbols: Vec::new(),
            persistent_xdr_keys: Vec::new(),
            persistent_key_exprs: Vec::new(),
            temporary_symbols: Vec::new(),
            temporary_xdr_keys: Vec::new(),
            threshold_config: ThresholdConfig::default(),
            instance_size_thresholds: InstanceSizeThresholds::default(),
            network: None,
            ledger_seconds_override: None,
        }
    }

    /// Add persistent symbol keys to the request.
    pub fn with_persistent_symbols(mut self, symbols: Vec<String>) -> Self {
        self.persistent_symbols = symbols;
        self
    }

    /// Add persistent base64 XDR keys to the request.
    pub fn with_persistent_xdr_keys(mut self, xdr_keys: Vec<String>) -> Self {
        self.persistent_xdr_keys = xdr_keys;
        self
    }

    /// Add persistent typed key expressions to the request.
    pub fn with_persistent_key_exprs(mut self, exprs: Vec<String>) -> Self {
        self.persistent_key_exprs = exprs;
        self
    }

    /// Add temporary symbol keys to the request.
    pub fn with_temporary_symbols(mut self, symbols: Vec<String>) -> Self {
        self.temporary_symbols = symbols;
        self
    }

    /// Add temporary base64 XDR keys to the request.
    pub fn with_temporary_xdr_keys(mut self, xdr_keys: Vec<String>) -> Self {
        self.temporary_xdr_keys = xdr_keys;
        self
    }

    /// Set a custom threshold configuration.
    pub fn with_threshold_config(mut self, threshold_config: ThresholdConfig) -> Self {
        self.threshold_config = threshold_config;
        self
    }

    /// Set custom instance-storage utilisation thresholds.
    pub fn with_instance_size_thresholds(mut self, thresholds: InstanceSizeThresholds) -> Self {
        self.instance_size_thresholds = thresholds;
        self
    }

    /// Set the network name.
    pub fn with_network(mut self, network: impl Into<String>) -> Self {
        self.network = Some(network.into());
        self
    }

    /// Set an optional ledger close velocity override.
    pub fn with_ledger_seconds(mut self, ledger_seconds: Option<f64>) -> Self {
        self.ledger_seconds_override = ledger_seconds;
        self
    }
}

/// Numerical summary counts of entries categorized by risk level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportSummary {
    /// Total count of inspected entries.
    pub total_entries: usize,
    /// Count of entries with `RiskLevel::Ok`.
    pub ok: usize,
    /// Count of entries with `RiskLevel::Warn`.
    pub warn: usize,
    /// Count of entries with `RiskLevel::Critical`.
    pub critical: usize,
    /// Count of entries with `RiskLevel::Archived`.
    pub archived: usize,
}

/// Individual report for an inspected ledger storage entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StorageEntryReport {
    /// Human-readable or base64 key identifier (e.g., "instance", "code_wasm", "persistent:COUNTER").
    #[serde(alias = "identifier")]
    pub key_identifier: String,
    /// Entry type classification (ContractInstance, ContractCode, PersistentData, TemporaryData).
    pub entry_type: EntryType,
    /// Durability tier (Instance, Code, Persistent, Temporary).
    pub durability: Durability,
    /// Byte size of the entry.
    #[serde(alias = "size_bytes")]
    pub byte_size: usize,
    /// Live-until ledger sequence number.
    pub live_until_ledger: u32,
    /// Remaining ledgers until expiration.
    pub remaining_ledgers: i64,
    /// Estimated wall-clock seconds remaining until expiration.
    pub estimated_remaining_seconds: f64,
    /// Estimated wall-clock days remaining until expiration.
    #[serde(alias = "estimated_remaining_days")]
    pub remaining_days: f64,
    /// Risk status classification.
    pub status: RiskLevel,
    /// Instance-storage size metrics; present only on a found contract instance entry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance_storage: Option<InstanceStorageReport>,
}

/// Instance-storage size metrics for a contract instance entry.
///
/// Instance storage lives in a single ledger entry, so it fails differently from TTL expiry:
/// once the entry reaches the network's maximum contract-data entry size, writes start failing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InstanceStorageReport {
    /// Number of keys in the instance storage map.
    pub key_count: u32,
    /// Serialized size of the instance ledger entry in bytes.
    pub entry_size_bytes: usize,
    /// Network `max_contract_data_entry_bytes`, or `None` if it could not be fetched.
    pub max_entry_size_bytes: Option<u32>,
    /// `entry_size_bytes` as a percentage of `max_entry_size_bytes`; `None` without the limit.
    pub utilization_pct: Option<f64>,
    /// Status from entry size alone (`Ok` when utilisation is unknown). The entry's overall
    /// `status` is the worse of this and its TTL status.
    pub size_status: RiskLevel,
}

impl StorageEntryReport {
    /// Create a new `StorageEntryReport`.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        key_identifier: impl Into<String>,
        entry_type: EntryType,
        durability: Durability,
        byte_size: usize,
        live_until_ledger: u32,
        remaining_ledgers: i64,
        estimated_remaining_seconds: f64,
        remaining_days: f64,
        status: RiskLevel,
    ) -> Self {
        Self {
            key_identifier: key_identifier.into(),
            entry_type,
            durability,
            byte_size,
            live_until_ledger,
            remaining_ledgers,
            estimated_remaining_seconds,
            remaining_days,
            status,
            instance_storage: None,
        }
    }

    /// Whether the entry is gone for good: a temporary entry that is missing or expired is
    /// deleted permanently and, unlike persistent data, cannot be restored.
    pub fn is_unrecoverable(&self) -> bool {
        self.durability == Durability::Temporary && self.status == RiskLevel::Archived
    }
}

/// Comprehensive contract storage TTL and risk evaluation report.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TTLReport {
    /// Target contract address strkey.
    pub contract_id: String,
    /// Stellar network inspected (e.g., "testnet", "mainnet").
    pub network: String,
    /// Sequence number of the latest ledger closed during inspection.
    pub latest_ledger: u32,
    /// Estimated seconds per ledger close (typically 5.0s).
    pub ledger_velocity_seconds: f64,
    /// Individual reports for all monitored storage entries.
    pub entries: Vec<StorageEntryReport>,
    /// Aggregate overall risk status for the contract.
    pub overall_status: RiskLevel,
    /// Advisory projections for eligible persistent entries.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub archival_recommendations: Vec<ArchivalRecommendation>,
}

impl TTLReport {
    /// Calculates aggregate summary counts across all evaluated entries.
    pub fn summary(&self) -> ReportSummary {
        let mut ok = 0;
        let mut warn = 0;
        let mut critical = 0;
        let mut archived = 0;

        for entry in &self.entries {
            match entry.status {
                RiskLevel::Ok => ok += 1,
                RiskLevel::Warn => warn += 1,
                RiskLevel::Critical => critical += 1,
                RiskLevel::Archived => archived += 1,
            }
        }

        ReportSummary {
            total_entries: self.entries.len(),
            ok,
            warn,
            critical,
            archived,
        }
    }
}

/// Computes the aggregate risk status across entries.
///
/// Priority order (highest risk wins):
/// 1. If any entry is `Archived` -> `Archived`
/// 2. If any entry is `Critical` -> `Critical`
/// 3. If any entry is `Warn` -> `Warn`
/// 4. Otherwise -> `Ok`
pub fn compute_overall_status(entries: &[StorageEntryReport]) -> RiskLevel {
    if entries.is_empty() {
        return RiskLevel::Ok;
    }
    if entries.iter().any(|e| e.status == RiskLevel::Archived) {
        RiskLevel::Archived
    } else if entries.iter().any(|e| e.status == RiskLevel::Critical) {
        RiskLevel::Critical
    } else if entries.iter().any(|e| e.status == RiskLevel::Warn) {
        RiskLevel::Warn
    } else {
        RiskLevel::Ok
    }
}

/// Builds instance-storage metrics, classifying utilisation when the network limit is known.
fn instance_storage_report(
    key_count: u32,
    entry_size_bytes: usize,
    max_entry_size_bytes: Option<u32>,
    thresholds: &InstanceSizeThresholds,
) -> InstanceStorageReport {
    let utilization_pct = max_entry_size_bytes
        .filter(|&limit| limit > 0)
        .map(|limit| entry_size_bytes as f64 * 100.0 / f64::from(limit));
    InstanceStorageReport {
        key_count,
        entry_size_bytes,
        max_entry_size_bytes,
        utilization_pct,
        size_status: utilization_pct.map_or(RiskLevel::Ok, |pct| thresholds.evaluate(pct)),
    }
}

/// Safely computes the binary byte size from base64 XDR string.
fn compute_byte_size(entry_xdr_base64: &str) -> usize {
    if let Ok(entry) =
        stellar_xdr::curr::LedgerEntry::from_xdr_base64(entry_xdr_base64, safe_xdr_limits())
    {
        if let Ok(bytes) = entry.to_xdr(safe_xdr_limits()) {
            return bytes.len();
        }
    }
    if let Ok(data) =
        stellar_xdr::curr::LedgerEntryData::from_xdr_base64(entry_xdr_base64, safe_xdr_limits())
    {
        if let Ok(bytes) = data.to_xdr(safe_xdr_limits()) {
            return bytes.len();
        }
    }
    // Fallback: estimate decoded length from base64 string
    let trimmed = entry_xdr_base64.trim();
    if trimmed.is_empty() {
        return 0;
    }
    let padding = trimmed.chars().rev().take(2).filter(|&c| c == '=').count();
    let decoded = (trimmed.len() * 3) / 4;
    if decoded > padding {
        decoded - padding
    } else {
        trimmed.len()
    }
}

/// A contract-data key to inspect, with the report identity it is published under.
struct DataKeyDescriptor {
    ident: String,
    key_xdr: String,
    entry_type: EntryType,
    durability: Durability,
}

impl DataKeyDescriptor {
    fn persistent(ident: String, key_xdr: String) -> Self {
        Self {
            ident,
            key_xdr,
            entry_type: EntryType::PersistentData,
            durability: Durability::Persistent,
        }
    }

    fn temporary(ident: String, key_xdr: String) -> Self {
        Self {
            ident,
            key_xdr,
            entry_type: EntryType::TemporaryData,
            durability: Durability::Temporary,
        }
    }
}

/// Application orchestrator for scanning Soroban smart contract storage TTL and expiration risk.
pub struct ScanContractUseCase<R: LedgerReader> {
    reader: R,
    archival: Option<(ArchivalPolicy, HashMap<String, ExtensionHistory>)>,
}

impl<R: LedgerReader> ScanContractUseCase<R> {
    /// Creates a new `ScanContractUseCase` with the given reader.
    pub fn new(reader: R) -> Self {
        Self {
            reader,
            archival: None,
        }
    }

    /// Enable advisory analysis with complete histories keyed by canonical ledger-key XDR.
    /// Histories and fee estimates must belong to the network being scanned.
    pub fn with_archival_analysis(
        mut self,
        policy: ArchivalPolicy,
        histories: HashMap<String, ExtensionHistory>,
    ) -> Self {
        self.archival = Some((policy, histories));
        self
    }

    /// Returns a reference to the underlying reader.
    pub fn reader(&self) -> &R {
        &self.reader
    }

    /// Executes the full contract scan workflow.
    pub async fn execute(&self, request: ScanRequest) -> SentinelResult<TTLReport> {
        let contract_id_bytes = parse_contract_id(&request.contract_id_strkey)?;
        request.threshold_config.validate()?;
        request.instance_size_thresholds.validate()?;
        let latest_ledger_info = self.reader.get_latest_ledger().await?;
        let latest_ledger = latest_ledger_info.sequence;

        let network = request
            .network
            .clone()
            .unwrap_or_else(|| "testnet".to_string());

        // 1. Derive deterministic ContractInstance key
        let instance_key_xdr = derive_contract_instance_key(&contract_id_bytes)?;

        // Resolve ledger close velocity: explicit CLI override, or query getLedgers from RPC, or default 5.0s
        let ledger_velocity = if let Some(ov) = request.ledger_seconds_override {
            if (1.0..=60.0).contains(&ov) {
                ov
            } else {
                TARGET_LEDGER_CLOSE_SECONDS
            }
        } else {
            match self.reader.get_ledger_close_velocity(latest_ledger).await {
                Ok(Some(v)) if (1.0..=60.0).contains(&v) => v,
                _ => TARGET_LEDGER_CLOSE_SECONDS,
            }
        };

        // 2. Derive contract-data keys: persistent keys from symbols, base64 ScVal, and typed
        // key expressions, then temporary keys from symbols and base64 ScVal.
        let mut data_descriptors: Vec<DataKeyDescriptor> = Vec::new();
        for sym in &request.persistent_symbols {
            let key_xdr = derive_persistent_symbol_key(&contract_id_bytes, sym)?;
            data_descriptors.push(DataKeyDescriptor::persistent(
                format!("persistent:{}", sym),
                key_xdr,
            ));
        }
        for xdr_key in &request.persistent_xdr_keys {
            let key_xdr = derive_persistent_xdr_key(&contract_id_bytes, xdr_key)?;
            data_descriptors.push(DataKeyDescriptor::persistent(
                format!("persistent_xdr:{}", xdr_key),
                key_xdr,
            ));
        }
        for expr in &request.persistent_key_exprs {
            let scval = parse_key_expr(expr)?;
            let key_xdr = derive_persistent_key_from_scval(&contract_id_bytes, scval)?;
            data_descriptors.push(DataKeyDescriptor::persistent(
                format!("persistent:{}", expr),
                key_xdr,
            ));
        }
        for sym in &request.temporary_symbols {
            let key_xdr = derive_temporary_symbol_key(&contract_id_bytes, sym)?;
            data_descriptors.push(DataKeyDescriptor::temporary(
                format!("temporary:{}", sym),
                key_xdr,
            ));
        }
        for xdr_key in &request.temporary_xdr_keys {
            let key_xdr = derive_temporary_xdr_key(&contract_id_bytes, xdr_key)?;
            data_descriptors.push(DataKeyDescriptor::temporary(
                format!("temporary_xdr:{}", xdr_key),
                key_xdr,
            ));
        }

        // 3. Query initial batch: instance key + every contract-data key + the network's
        // maximum contract-data entry size (a config-setting entry, so no extra RPC call)
        let entry_size_limit_key = derive_contract_data_entry_size_limit_key()?;
        let mut query_keys: Vec<String> = Vec::with_capacity(2 + data_descriptors.len());
        query_keys.push(instance_key_xdr.clone());
        for descriptor in &data_descriptors {
            query_keys.push(descriptor.key_xdr.clone());
        }
        query_keys.push(entry_size_limit_key.clone());

        let raw_entries = self.reader.get_ledger_entries(&query_keys).await?;
        let max_entry_size_bytes = raw_entries
            .iter()
            .find(|e| e.key_xdr_base64 == entry_size_limit_key)
            .and_then(|e| extract_contract_data_entry_size_limit(&e.entry_xdr_base64).ok());

        let mut reports = Vec::new();

        // 4. Process ContractInstance entry
        let instance_entry_opt = raw_entries
            .iter()
            .find(|e| e.key_xdr_base64 == instance_key_xdr);

        let mut wasm_hash_opt: Option<[u8; 32]> = None;

        if let Some(entry) = instance_entry_opt {
            let byte_size = compute_byte_size(&entry.entry_xdr_base64);
            let live_until = entry.live_until_ledger_seq;
            let remaining_ledgers = calculate_remaining_ledgers(live_until, latest_ledger);
            let remaining_seconds =
                estimate_remaining_seconds_with_velocity(remaining_ledgers, ledger_velocity);
            let remaining_days = estimate_remaining_days(remaining_seconds);
            let ttl_status = evaluate_risk(remaining_ledgers, &request.threshold_config);

            // Malformed storage leaves size metrics out rather than failing the scan.
            let instance_storage = extract_instance_storage_stats(&entry.entry_xdr_base64)
                .ok()
                .map(|stats| {
                    instance_storage_report(
                        stats.key_count,
                        stats.entry_size_bytes,
                        max_entry_size_bytes,
                        &request.instance_size_thresholds,
                    )
                });
            let status = instance_storage
                .as_ref()
                .map_or(ttl_status, |metrics| ttl_status.max(metrics.size_status));

            let mut report = StorageEntryReport::new(
                "instance",
                EntryType::ContractInstance,
                Durability::Instance,
                byte_size,
                live_until,
                remaining_ledgers,
                remaining_seconds,
                remaining_days,
                status,
            );
            report.instance_storage = instance_storage;
            reports.push(report);

            // Extract WASM hash if executable is WASM bytecode
            wasm_hash_opt = extract_wasm_hash_from_instance_entry(&entry.entry_xdr_base64)
                .ok()
                .flatten();
        } else {
            // Missing/archived instance entry
            reports.push(StorageEntryReport::new(
                "instance",
                EntryType::ContractInstance,
                Durability::Instance,
                0,
                0,
                0,
                0.0,
                0.0,
                RiskLevel::Archived,
            ));
        }

        // 5. Process ContractCode entry (if WASM executable exists)
        if let Some(wasm_hash) = wasm_hash_opt {
            let code_key_xdr = derive_contract_code_key(&wasm_hash)?;
            let code_entries = self
                .reader
                .get_ledger_entries(std::slice::from_ref(&code_key_xdr))
                .await?;
            let code_entry_opt = code_entries
                .into_iter()
                .find(|e| e.key_xdr_base64 == code_key_xdr);

            if let Some(code_entry) = code_entry_opt {
                let byte_size = compute_byte_size(&code_entry.entry_xdr_base64);
                let live_until = code_entry.live_until_ledger_seq;
                let remaining_ledgers = calculate_remaining_ledgers(live_until, latest_ledger);
                let remaining_seconds =
                    estimate_remaining_seconds_with_velocity(remaining_ledgers, ledger_velocity);
                let remaining_days = estimate_remaining_days(remaining_seconds);
                let status = evaluate_risk(remaining_ledgers, &request.threshold_config);

                reports.push(StorageEntryReport::new(
                    "code_wasm",
                    EntryType::ContractCode,
                    Durability::Code,
                    byte_size,
                    live_until,
                    remaining_ledgers,
                    remaining_seconds,
                    remaining_days,
                    status,
                ));
            } else {
                // Missing/archived code entry
                reports.push(StorageEntryReport::new(
                    "code_wasm",
                    EntryType::ContractCode,
                    Durability::Code,
                    0,
                    0,
                    0,
                    0.0,
                    0.0,
                    RiskLevel::Archived,
                ));
            }
        }

        let mut archival_recommendations = Vec::new();
        let mut evaluated_keys = HashSet::new();

        // 6. Process contract-data entries (persistent and temporary)
        for descriptor in data_descriptors {
            let DataKeyDescriptor {
                ident,
                key_xdr,
                entry_type,
                durability,
            } = descriptor;
            let found_entry = raw_entries.iter().find(|e| e.key_xdr_base64 == key_xdr);

            if let Some(entry) = found_entry {
                let byte_size = compute_byte_size(&entry.entry_xdr_base64);
                let live_until = entry.live_until_ledger_seq;
                let remaining_ledgers = calculate_remaining_ledgers(live_until, latest_ledger);
                let remaining_seconds =
                    estimate_remaining_seconds_with_velocity(remaining_ledgers, ledger_velocity);
                let remaining_days = estimate_remaining_days(remaining_seconds);
                let status = evaluate_risk(remaining_ledgers, &request.threshold_config);

                // Archival advice applies only to persistent data: temporary entries are
                // deleted on expiry and cannot be restored.
                let archival = self
                    .archival
                    .as_ref()
                    .filter(|_| durability == Durability::Persistent);
                if let Some((policy, histories)) = archival {
                    if let Some(history) = histories.get(&key_xdr) {
                        if key_xdr != instance_key_xdr && evaluated_keys.insert(key_xdr) {
                            let profile = StorageEntry::new(
                                &ident,
                                EntryType::PersistentData,
                                Durability::Persistent,
                                byte_size,
                                live_until,
                                entry.last_modified_ledger_seq,
                            );
                            archival_recommendations.extend(recommend_archival(
                                &profile,
                                *history,
                                policy,
                                latest_ledger,
                                ledger_velocity,
                            ));
                        }
                    }
                }

                reports.push(StorageEntryReport::new(
                    ident,
                    entry_type,
                    durability,
                    byte_size,
                    live_until,
                    remaining_ledgers,
                    remaining_seconds,
                    remaining_days,
                    status,
                ));
            } else {
                // Missing/archived entry (a missing temporary entry is permanently deleted)
                reports.push(StorageEntryReport::new(
                    ident,
                    entry_type,
                    durability,
                    0,
                    0,
                    0,
                    0.0,
                    0.0,
                    RiskLevel::Archived,
                ));
            }
        }

        let overall_status = compute_overall_status(&reports);

        Ok(TTLReport {
            contract_id: request.contract_id_strkey,
            network,
            latest_ledger,
            ledger_velocity_seconds: ledger_velocity,
            entries: reports,
            overall_status,
            archival_recommendations,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use stellar_xdr::curr::{
        ContractCodeEntry, ContractCodeEntryExt, ContractDataDurability, ContractDataEntry,
        ContractExecutable, ExtensionPoint, Hash, LedgerEntryData, Limits, ScAddress,
        ScContractInstance, ScSymbol, ScVal,
    };

    use crate::ports::mock::MockLedgerReader;
    use crate::ports::reader::RawLedgerEntry;

    fn make_test_instance_xdr(contract_id: [u8; 32], wasm_hash: [u8; 32]) -> String {
        let entry_data = LedgerEntryData::ContractData(ContractDataEntry {
            ext: ExtensionPoint::V0,
            contract: ScAddress::Contract(Hash(contract_id)),
            key: ScVal::LedgerKeyContractInstance,
            durability: ContractDataDurability::Persistent,
            val: ScVal::ContractInstance(ScContractInstance {
                executable: ContractExecutable::Wasm(Hash(wasm_hash)),
                storage: None,
            }),
        });
        entry_data.to_xdr_base64(Limits::none()).unwrap()
    }

    fn make_test_stellar_asset_instance_xdr(contract_id: [u8; 32]) -> String {
        LedgerEntryData::ContractData(ContractDataEntry {
            ext: ExtensionPoint::V0,
            contract: ScAddress::Contract(Hash(contract_id)),
            key: ScVal::LedgerKeyContractInstance,
            durability: ContractDataDurability::Persistent,
            val: ScVal::ContractInstance(ScContractInstance {
                executable: ContractExecutable::StellarAsset,
                storage: None,
            }),
        })
        .to_xdr_base64(Limits::none())
        .unwrap()
    }

    fn make_test_code_xdr(wasm_hash: [u8; 32], code_bytes: &[u8]) -> String {
        let entry_data = LedgerEntryData::ContractCode(ContractCodeEntry {
            ext: ContractCodeEntryExt::V0,
            hash: Hash(wasm_hash),
            code: code_bytes.to_vec().try_into().unwrap(),
        });
        entry_data.to_xdr_base64(Limits::none()).unwrap()
    }

    fn make_test_persistent_symbol_xdr(
        contract_id: [u8; 32],
        symbol: &str,
        val_u32: u32,
    ) -> String {
        let entry_data = LedgerEntryData::ContractData(ContractDataEntry {
            ext: ExtensionPoint::V0,
            contract: ScAddress::Contract(Hash(contract_id)),
            key: ScVal::Symbol(ScSymbol(symbol.try_into().unwrap())),
            durability: ContractDataDurability::Persistent,
            val: ScVal::U32(val_u32),
        });
        entry_data.to_xdr_base64(Limits::none()).unwrap()
    }

    #[tokio::test]
    async fn test_scan_happy_path_with_wasm_and_persistent_keys() {
        let contract_bytes = [7u8; 32];
        let wasm_bytes = [9u8; 32];
        let contract_strkey = stellar_strkey::Contract(contract_bytes).to_string();

        let instance_key = derive_contract_instance_key(&contract_bytes).unwrap();
        let code_key = derive_contract_code_key(&wasm_bytes).unwrap();
        let symbol_key = derive_persistent_symbol_key(&contract_bytes, "COUNTER").unwrap();

        let instance_xdr = make_test_instance_xdr(contract_bytes, wasm_bytes);
        let code_xdr = make_test_code_xdr(wasm_bytes, &[0xDE, 0xAD, 0xBE, 0xEF]);
        let symbol_xdr = make_test_persistent_symbol_xdr(contract_bytes, "COUNTER", 42);

        let latest_ledger = 5_400_000;
        // Instance: 5_550_000 -> 150_000 ledgers remaining -> OK (> 120_960)
        let instance_raw = RawLedgerEntry::new(&instance_key, &instance_xdr, 5_550_000, 5_390_000);
        // Code: 5_500_000 -> 100_000 ledgers remaining -> WARN (< 120_960 and >= 34_560)
        let code_raw = RawLedgerEntry::new(&code_key, &code_xdr, 5_500_000, 5_380_000);
        // Symbol: 5_800_000 -> 400_000 ledgers remaining -> OK
        let symbol_raw = RawLedgerEntry::new(&symbol_key, &symbol_xdr, 5_800_000, 5_350_000);

        let mock_reader = MockLedgerReader::new()
            .with_latest_ledger(latest_ledger, 22)
            .with_entry(instance_raw)
            .with_entry(code_raw)
            .with_entry(symbol_raw);

        let usecase = ScanContractUseCase::new(mock_reader);
        let request = ScanRequest::new(&contract_strkey)
            .with_persistent_symbols(vec!["COUNTER".to_string()])
            .with_network("testnet");

        let report = usecase.execute(request).await.expect("Scan should succeed");

        assert_eq!(report.contract_id, contract_strkey);
        assert_eq!(report.network, "testnet");
        assert_eq!(report.latest_ledger, 5_400_000);
        assert_eq!(report.ledger_velocity_seconds, 5.0);
        assert_eq!(report.entries.len(), 3);

        // 1. Instance Entry
        let inst = &report.entries[0];
        assert_eq!(inst.key_identifier, "instance");
        assert_eq!(inst.entry_type, EntryType::ContractInstance);
        assert_eq!(inst.durability, Durability::Instance);
        assert_eq!(inst.live_until_ledger, 5_550_000);
        assert_eq!(inst.remaining_ledgers, 150_000);
        assert_eq!(inst.status, RiskLevel::Ok);
        assert!(inst.byte_size > 0);

        // 2. Code Entry
        let code = &report.entries[1];
        assert_eq!(code.key_identifier, "code_wasm");
        assert_eq!(code.entry_type, EntryType::ContractCode);
        assert_eq!(code.durability, Durability::Code);
        assert_eq!(code.live_until_ledger, 5_500_000);
        assert_eq!(code.remaining_ledgers, 100_000);
        assert_eq!(code.status, RiskLevel::Warn);
        assert!(code.byte_size > 0);

        // 3. Persistent Symbol Entry
        let sym = &report.entries[2];
        assert_eq!(sym.key_identifier, "persistent:COUNTER");
        assert_eq!(sym.entry_type, EntryType::PersistentData);
        assert_eq!(sym.durability, Durability::Persistent);
        assert_eq!(sym.live_until_ledger, 5_800_000);
        assert_eq!(sym.remaining_ledgers, 400_000);
        assert_eq!(sym.status, RiskLevel::Ok);

        // Overall status should escalate to WARN because code is WARN
        assert_eq!(report.overall_status, RiskLevel::Warn);

        // Summary
        let summary = report.summary();
        assert_eq!(summary.total_entries, 3);
        assert_eq!(summary.ok, 2);
        assert_eq!(summary.warn, 1);
        assert_eq!(summary.critical, 0);
        assert_eq!(summary.archived, 0);
    }

    #[tokio::test]
    async fn test_scan_contract_instance_missing_marks_archived() {
        let contract_bytes = [12u8; 32];
        let contract_strkey = stellar_strkey::Contract(contract_bytes).to_string();

        let mock_reader = MockLedgerReader::new().with_latest_ledger(5_400_000, 22);
        let usecase = ScanContractUseCase::new(mock_reader);
        let request = ScanRequest::new(&contract_strkey);

        let report = usecase.execute(request).await.expect("Scan should succeed");

        assert_eq!(report.entries.len(), 1);
        let inst = &report.entries[0];
        assert_eq!(inst.key_identifier, "instance");
        assert_eq!(inst.status, RiskLevel::Archived);
        assert_eq!(inst.live_until_ledger, 0);
        assert_eq!(inst.remaining_ledgers, 0);
        assert_eq!(report.overall_status, RiskLevel::Archived);

        let summary = report.summary();
        assert_eq!(summary.archived, 1);
        assert_eq!(summary.ok, 0);
    }

    #[tokio::test]
    async fn test_scan_code_entry_missing_marks_archived() {
        let contract_bytes = [15u8; 32];
        let wasm_bytes = [16u8; 32];
        let contract_strkey = stellar_strkey::Contract(contract_bytes).to_string();

        let instance_key = derive_contract_instance_key(&contract_bytes).unwrap();
        let instance_xdr = make_test_instance_xdr(contract_bytes, wasm_bytes);

        // Instance entry exists (OK), but code entry is omitted
        let instance_raw = RawLedgerEntry::new(&instance_key, &instance_xdr, 6_000_000, 5_400_000);

        let mock_reader = MockLedgerReader::new()
            .with_latest_ledger(5_400_000, 22)
            .with_entry(instance_raw);

        let usecase = ScanContractUseCase::new(mock_reader);
        let request = ScanRequest::new(&contract_strkey);

        let report = usecase.execute(request).await.expect("Scan should succeed");

        assert_eq!(report.entries.len(), 2);
        assert_eq!(report.entries[0].key_identifier, "instance");
        assert_eq!(report.entries[0].status, RiskLevel::Ok);

        assert_eq!(report.entries[1].key_identifier, "code_wasm");
        assert_eq!(report.entries[1].status, RiskLevel::Archived);
        assert_eq!(report.entries[1].live_until_ledger, 0);

        // Overall status should be Archived due to missing code
        assert_eq!(report.overall_status, RiskLevel::Archived);
    }

    #[tokio::test]
    async fn test_scan_invalid_contract_address_returns_error() {
        let mock_reader = MockLedgerReader::new();
        let usecase = ScanContractUseCase::new(mock_reader);
        let request = ScanRequest::new("INVALID_STRKEY_NOT_CONTRACT");

        let result = usecase.execute(request).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_scan_stellar_asset_contract_has_no_code_wasm_entry() {
        let contract_bytes = [20u8; 32];
        let contract_strkey = stellar_strkey::Contract(contract_bytes).to_string();

        let instance_key = derive_contract_instance_key(&contract_bytes).unwrap();
        let entry_data = LedgerEntryData::ContractData(ContractDataEntry {
            ext: ExtensionPoint::V0,
            contract: ScAddress::Contract(Hash(contract_bytes)),
            key: ScVal::LedgerKeyContractInstance,
            durability: ContractDataDurability::Persistent,
            val: ScVal::ContractInstance(ScContractInstance {
                executable: ContractExecutable::StellarAsset,
                storage: None,
            }),
        });
        let instance_xdr = entry_data.to_xdr_base64(Limits::none()).unwrap();

        let instance_raw = RawLedgerEntry::new(&instance_key, &instance_xdr, 6_000_000, 5_400_000);
        let mock_reader = MockLedgerReader::new()
            .with_latest_ledger(5_400_000, 22)
            .with_entry(instance_raw);

        let usecase = ScanContractUseCase::new(mock_reader);
        let request = ScanRequest::new(&contract_strkey);

        let report = usecase.execute(request).await.unwrap();

        // Only instance should be present; StellarAsset has no WASM code entry
        assert_eq!(report.entries.len(), 1);
        assert_eq!(report.entries[0].key_identifier, "instance");
        assert_eq!(report.overall_status, RiskLevel::Ok);
    }

    #[tokio::test]
    async fn test_scan_with_persistent_xdr_key() {
        let contract_bytes = [30u8; 32];
        let contract_strkey = stellar_strkey::Contract(contract_bytes).to_string();

        let instance_key = derive_contract_instance_key(&contract_bytes).unwrap();
        let instance_xdr = make_test_instance_xdr(contract_bytes, [31u8; 32]);
        let instance_raw = RawLedgerEntry::new(&instance_key, &instance_xdr, 5_600_000, 5_400_000);

        // Persistent ScVal key: ScVal::U32(99)
        let scval = ScVal::U32(99);
        let scval_xdr = scval.to_xdr_base64(Limits::none()).unwrap();
        let persistent_key = derive_persistent_xdr_key(&contract_bytes, &scval_xdr).unwrap();

        let persistent_entry_data = LedgerEntryData::ContractData(ContractDataEntry {
            ext: ExtensionPoint::V0,
            contract: ScAddress::Contract(Hash(contract_bytes)),
            key: scval,
            durability: ContractDataDurability::Persistent,
            val: ScVal::U32(100),
        });
        let persistent_entry_xdr = persistent_entry_data.to_xdr_base64(Limits::none()).unwrap();
        // Live until 5_420_000 -> 20_000 remaining (< 34_560) -> Critical
        let persistent_raw =
            RawLedgerEntry::new(&persistent_key, &persistent_entry_xdr, 5_420_000, 5_400_000);

        let mock_reader = MockLedgerReader::new()
            .with_latest_ledger(5_400_000, 22)
            .with_entry(instance_raw)
            .with_entry(persistent_raw);

        let usecase = ScanContractUseCase::new(mock_reader);
        let request =
            ScanRequest::new(&contract_strkey).with_persistent_xdr_keys(vec![scval_xdr.clone()]);

        let report = usecase.execute(request).await.unwrap();

        assert_eq!(report.overall_status, RiskLevel::Archived); // code_wasm missing -> archived
        assert!(report
            .entries
            .iter()
            .any(|e| e.status == RiskLevel::Critical));
    }

    #[tokio::test]
    async fn test_scan_temporary_keys_found_and_missing_alongside_persistent() {
        let contract_bytes = [60u8; 32];
        let contract_strkey = stellar_strkey::Contract(contract_bytes).to_string();

        let instance_key = derive_contract_instance_key(&contract_bytes).unwrap();
        let instance_entry = RawLedgerEntry::new(
            &instance_key,
            make_test_stellar_asset_instance_xdr(contract_bytes),
            5_600_000,
            5_400_000,
        );

        // Same symbol in both tiers: they are distinct ledger entries.
        let persistent_key = derive_persistent_symbol_key(&contract_bytes, "NONCE").unwrap();
        let temporary_key =
            crate::adapters::derive_temporary_symbol_key(&contract_bytes, "NONCE").unwrap();
        assert_ne!(persistent_key, temporary_key);
        let temporary_entry_xdr = LedgerEntryData::ContractData(ContractDataEntry {
            ext: ExtensionPoint::V0,
            contract: ScAddress::Contract(Hash(contract_bytes)),
            key: ScVal::Symbol(ScSymbol("NONCE".try_into().unwrap())),
            durability: ContractDataDurability::Temporary,
            val: ScVal::U64(1),
        })
        .to_xdr_base64(Limits::none())
        .unwrap();
        let persistent_entry = RawLedgerEntry::new(
            &persistent_key,
            make_test_persistent_symbol_xdr(contract_bytes, "NONCE", 3),
            5_700_000,
            5_400_000,
        );
        // 50_000 ledgers remaining -> WARN
        let temporary_entry =
            RawLedgerEntry::new(&temporary_key, &temporary_entry_xdr, 5_450_000, 5_400_000);

        let missing_scval = ScVal::U32(404).to_xdr_base64(Limits::none()).unwrap();

        let mock_reader = MockLedgerReader::new()
            .with_latest_ledger(5_400_000, 22)
            .with_entry(instance_entry)
            .with_entry(persistent_entry)
            .with_entry(temporary_entry);
        let usecase = ScanContractUseCase::new(mock_reader);
        let request = ScanRequest::new(&contract_strkey)
            .with_persistent_symbols(vec!["NONCE".to_string()])
            .with_temporary_symbols(vec!["NONCE".to_string()])
            .with_temporary_xdr_keys(vec![missing_scval.clone()]);

        let report = usecase.execute(request).await.unwrap();
        let ids: Vec<&str> = report
            .entries
            .iter()
            .map(|e| e.key_identifier.as_str())
            .collect();
        assert_eq!(
            ids,
            vec![
                "instance".to_string(),
                "persistent:NONCE".to_string(),
                "temporary:NONCE".to_string(),
                format!("temporary_xdr:{}", missing_scval),
            ]
        );

        let persistent = &report.entries[1];
        assert_eq!(persistent.durability, Durability::Persistent);
        assert_eq!(persistent.live_until_ledger, 5_700_000);

        let found = &report.entries[2];
        assert_eq!(found.entry_type, EntryType::TemporaryData);
        assert_eq!(found.durability, Durability::Temporary);
        assert_eq!(found.live_until_ledger, 5_450_000);
        assert_eq!(found.remaining_ledgers, 50_000);
        assert_eq!(found.status, RiskLevel::Warn);
        assert!(found.byte_size > 0);

        let missing = &report.entries[3];
        assert_eq!(missing.entry_type, EntryType::TemporaryData);
        assert_eq!(missing.durability, Durability::Temporary);
        assert_eq!(missing.live_until_ledger, 0);
        assert_eq!(missing.status, RiskLevel::Archived);
        assert_eq!(report.overall_status, RiskLevel::Archived);
    }

    #[tokio::test]
    async fn test_scan_rejects_invalid_temporary_keys() {
        let contract_strkey = stellar_strkey::Contract([61u8; 32]).to_string();
        let usecase =
            ScanContractUseCase::new(MockLedgerReader::new().with_latest_ledger(5_400_000, 22));

        let bad_symbol =
            ScanRequest::new(&contract_strkey).with_temporary_symbols(vec!["a".repeat(33)]);
        assert!(matches!(
            usecase.execute(bad_symbol).await,
            Err(crate::error::SentinelError::InvalidXdrKey(_))
        ));

        let bad_xdr =
            ScanRequest::new(&contract_strkey).with_temporary_xdr_keys(vec!["%%%".to_string()]);
        assert!(matches!(
            usecase.execute(bad_xdr).await,
            Err(crate::error::SentinelError::InvalidXdrKey(_))
        ));
    }

    /// A Stellar Asset instance entry whose storage map holds `keys` byte values of `value_bytes`.
    fn make_instance_with_storage_xdr(
        contract_id: [u8; 32],
        keys: u32,
        value_bytes: usize,
    ) -> String {
        use stellar_xdr::curr::{ScBytes, ScMap, ScMapEntry};
        let entries: Vec<ScMapEntry> = (0..keys)
            .map(|i| ScMapEntry {
                key: ScVal::U32(i),
                val: ScVal::Bytes(ScBytes(vec![0x11; value_bytes].try_into().unwrap())),
            })
            .collect();
        LedgerEntryData::ContractData(ContractDataEntry {
            ext: ExtensionPoint::V0,
            contract: ScAddress::Contract(Hash(contract_id)),
            key: ScVal::LedgerKeyContractInstance,
            durability: ContractDataDurability::Persistent,
            val: ScVal::ContractInstance(ScContractInstance {
                executable: ContractExecutable::StellarAsset,
                storage: Some(ScMap(entries.try_into().unwrap())),
            }),
        })
        .to_xdr_base64(Limits::none())
        .unwrap()
    }

    fn entry_size_limit_raw(limit: u32) -> RawLedgerEntry {
        let entry = LedgerEntryData::ConfigSetting(
            stellar_xdr::curr::ConfigSettingEntry::ContractDataEntrySizeBytes(limit),
        )
        .to_xdr_base64(Limits::none())
        .unwrap();
        RawLedgerEntry::new(
            derive_contract_data_entry_size_limit_key().unwrap(),
            entry,
            0,
            0,
        )
    }

    async fn scan_instance(
        contract_bytes: [u8; 32],
        instance_xdr: &str,
        limit: Option<u32>,
    ) -> TTLReport {
        let instance_key = derive_contract_instance_key(&contract_bytes).unwrap();
        // 500,000 ledgers of TTL left: always OK on TTL alone.
        let mut reader = MockLedgerReader::new()
            .with_latest_ledger(5_400_000, 22)
            .with_entry(RawLedgerEntry::new(
                &instance_key,
                instance_xdr,
                5_900_000,
                5_400_000,
            ));
        if let Some(limit) = limit {
            reader = reader.with_entry(entry_size_limit_raw(limit));
        }
        ScanContractUseCase::new(reader)
            .execute(ScanRequest::new(
                stellar_strkey::Contract(contract_bytes).to_string(),
            ))
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn test_scan_reports_instance_storage_for_empty_and_small_maps() {
        let contract_bytes = [80u8; 32];
        let empty = scan_instance(
            contract_bytes,
            &make_test_stellar_asset_instance_xdr(contract_bytes),
            Some(65_536),
        )
        .await;
        let metrics = empty.entries[0].instance_storage.clone().unwrap();
        assert_eq!(metrics.key_count, 0);
        assert_eq!(metrics.max_entry_size_bytes, Some(65_536));
        assert!(metrics.utilization_pct.unwrap() < 1.0);
        assert_eq!(metrics.size_status, RiskLevel::Ok);
        assert_eq!(empty.entries[0].status, RiskLevel::Ok);

        let small = scan_instance(
            contract_bytes,
            &make_instance_with_storage_xdr(contract_bytes, 4, 16),
            Some(65_536),
        )
        .await;
        let metrics = small.entries[0].instance_storage.clone().unwrap();
        assert_eq!(metrics.key_count, 4);
        assert_eq!(metrics.size_status, RiskLevel::Ok);
        assert_eq!(small.overall_status, RiskLevel::Ok);
    }

    #[tokio::test]
    async fn test_scan_near_limit_instance_storage_raises_status() {
        let contract_bytes = [81u8; 32];
        let instance_xdr = make_instance_with_storage_xdr(contract_bytes, 60, 1_000);
        let size = extract_instance_storage_stats(&instance_xdr)
            .unwrap()
            .entry_size_bytes as u32;

        // size is 80% of the limit -> WARN (75 <= 80 < 90), though TTL is OK
        let warn = scan_instance(contract_bytes, &instance_xdr, Some(size * 5 / 4)).await;
        let instance = &warn.entries[0];
        let metrics = instance.instance_storage.clone().unwrap();
        assert_eq!(metrics.key_count, 60);
        assert_eq!(metrics.entry_size_bytes, size as usize);
        assert!((metrics.utilization_pct.unwrap() - 80.0).abs() < 0.01);
        assert_eq!(metrics.size_status, RiskLevel::Warn);
        assert_eq!(instance.status, RiskLevel::Warn);
        assert_eq!(warn.overall_status, RiskLevel::Warn);

        // the entry fills the whole limit -> CRITICAL
        let critical = scan_instance(contract_bytes, &instance_xdr, Some(size)).await;
        assert_eq!(
            critical.entries[0]
                .instance_storage
                .as_ref()
                .unwrap()
                .size_status,
            RiskLevel::Critical
        );
        assert_eq!(critical.overall_status, RiskLevel::Critical);
    }

    #[tokio::test]
    async fn test_scan_without_entry_size_limit_omits_utilization() {
        let contract_bytes = [82u8; 32];
        let report = scan_instance(
            contract_bytes,
            &make_instance_with_storage_xdr(contract_bytes, 60, 1_000),
            None,
        )
        .await;
        let metrics = report.entries[0].instance_storage.clone().unwrap();
        assert_eq!(metrics.key_count, 60);
        assert!(metrics.entry_size_bytes > 60_000);
        assert_eq!(metrics.max_entry_size_bytes, None);
        assert_eq!(metrics.utilization_pct, None);
        assert_eq!(metrics.size_status, RiskLevel::Ok);
        assert_eq!(report.overall_status, RiskLevel::Ok);
    }

    #[tokio::test]
    async fn test_scan_malformed_instance_storage_is_not_fatal() {
        let contract_bytes = [83u8; 32];
        // A contract-data entry at the instance key whose value is not an instance.
        let not_an_instance = LedgerEntryData::ContractData(ContractDataEntry {
            ext: ExtensionPoint::V0,
            contract: ScAddress::Contract(Hash(contract_bytes)),
            key: ScVal::LedgerKeyContractInstance,
            durability: ContractDataDurability::Persistent,
            val: ScVal::U32(1),
        })
        .to_xdr_base64(Limits::none())
        .unwrap();
        let report = scan_instance(contract_bytes, &not_an_instance, Some(65_536)).await;
        assert_eq!(report.entries[0].instance_storage, None);
        assert_eq!(report.entries[0].status, RiskLevel::Ok);
    }

    #[tokio::test]
    async fn test_scan_rejects_invalid_instance_size_thresholds_before_rpc() {
        // No latest ledger configured: reaching the RPC would fail differently.
        let usecase = ScanContractUseCase::new(MockLedgerReader::new());
        let request = ScanRequest::new(stellar_strkey::Contract([84u8; 32]).to_string())
            .with_instance_size_thresholds(InstanceSizeThresholds {
                warn_pct: 90,
                critical_pct: 75,
            });
        assert!(matches!(
            usecase.execute(request).await,
            Err(crate::error::SentinelError::InvalidThresholdConfig(_))
        ));
    }

    #[test]
    fn test_compute_overall_status_precedence() {
        let make_entry = |status: RiskLevel| {
            StorageEntryReport::new(
                "test",
                EntryType::ContractInstance,
                Durability::Instance,
                100,
                5_500_000,
                100_000,
                500_000.0,
                5.78,
                status,
            )
        };

        assert_eq!(compute_overall_status(&[]), RiskLevel::Ok);
        assert_eq!(
            compute_overall_status(&[make_entry(RiskLevel::Ok)]),
            RiskLevel::Ok
        );
        assert_eq!(
            compute_overall_status(&[make_entry(RiskLevel::Ok), make_entry(RiskLevel::Warn)]),
            RiskLevel::Warn
        );
        assert_eq!(
            compute_overall_status(&[
                make_entry(RiskLevel::Ok),
                make_entry(RiskLevel::Warn),
                make_entry(RiskLevel::Critical)
            ]),
            RiskLevel::Critical
        );
        assert_eq!(
            compute_overall_status(&[
                make_entry(RiskLevel::Ok),
                make_entry(RiskLevel::Critical),
                make_entry(RiskLevel::Archived)
            ]),
            RiskLevel::Archived
        );
    }

    #[tokio::test]
    async fn test_scan_with_persistent_key_expr() {
        let contract_bytes = [40u8; 32];
        let contract_strkey = stellar_strkey::Contract(contract_bytes).to_string();

        let instance_key = derive_contract_instance_key(&contract_bytes).unwrap();
        let instance_xdr = make_test_instance_xdr(contract_bytes, [41u8; 32]);
        let instance_raw = RawLedgerEntry::new(&instance_key, &instance_xdr, 5_600_000, 5_400_000);

        // Persistent key expr: vec[sym:Balance, u32:42]
        let key_expr = "vec[sym:Balance, u32:42]";
        let scval = parse_key_expr(key_expr).unwrap();
        let persistent_key =
            derive_persistent_key_from_scval(&contract_bytes, scval.clone()).unwrap();

        let persistent_entry_data = LedgerEntryData::ContractData(ContractDataEntry {
            ext: ExtensionPoint::V0,
            contract: ScAddress::Contract(Hash(contract_bytes)),
            key: scval,
            durability: ContractDataDurability::Persistent,
            val: ScVal::U32(100),
        });
        let persistent_entry_xdr = persistent_entry_data.to_xdr_base64(Limits::none()).unwrap();
        let persistent_raw =
            RawLedgerEntry::new(&persistent_key, &persistent_entry_xdr, 5_700_000, 5_400_000);

        let mock_reader = MockLedgerReader::new()
            .with_latest_ledger(5_400_000, 22)
            .with_entry(instance_raw)
            .with_entry(persistent_raw);

        let usecase = ScanContractUseCase::new(mock_reader);
        let request = ScanRequest::new(&contract_strkey)
            .with_persistent_key_exprs(vec![key_expr.to_string()]);

        let report = usecase.execute(request).await.unwrap();

        let expr_entry = report
            .entries
            .iter()
            .find(|e| e.key_identifier == "persistent:vec[sym:Balance, u32:42]")
            .expect("should find report with persistent:vec[sym:Balance, u32:42] identifier");
        assert_eq!(expr_entry.live_until_ledger, 5_700_000);
        assert_eq!(expr_entry.remaining_ledgers, 300_000);
        assert_eq!(expr_entry.status, RiskLevel::Ok);
    }

    #[tokio::test]
    async fn test_scan_with_measured_ledger_velocity() {
        let contract_bytes = [50u8; 32];
        let contract_strkey = stellar_strkey::Contract(contract_bytes).to_string();

        let instance_key = derive_contract_instance_key(&contract_bytes).unwrap();
        let instance_xdr = make_test_instance_xdr(contract_bytes, [51u8; 32]);
        let instance_raw = RawLedgerEntry::new(&instance_key, &instance_xdr, 5_500_000, 5_400_000);

        // Measured velocity = 6.0s/ledger
        let mock_reader = MockLedgerReader::new()
            .with_latest_ledger(5_400_000, 22)
            .with_velocity(6.0)
            .with_entry(instance_raw);

        let usecase = ScanContractUseCase::new(mock_reader);
        let request = ScanRequest::new(&contract_strkey);

        let report = usecase.execute(request).await.unwrap();
        assert_eq!(report.ledger_velocity_seconds, 6.0);

        let inst = &report.entries[0];
        // 100,000 ledgers * 6.0s = 600,000s
        assert_eq!(inst.estimated_remaining_seconds, 600_000.0);
        // 600,000 / 86400 ~= 6.9444 days
        assert!((inst.remaining_days - 6.9444).abs() < 1e-3);
    }
}
