//! Use case orchestrator for scanning Soroban smart contract storage TTL and expiration risk.

use serde::{Deserialize, Serialize};
use stellar_xdr::curr::{ReadXdr, WriteXdr};

use crate::adapters::{
    derive_contract_code_key, derive_contract_instance_key, derive_persistent_symbol_key,
    derive_persistent_xdr_key, extract_wasm_hash_from_instance_entry, parse_contract_id,
    safe_xdr_limits,
};
use crate::domain::{
    calculate_remaining_ledgers, estimate_remaining_days, estimate_remaining_seconds,
    evaluate_risk, Durability, EntryType, RiskLevel, ThresholdConfig, TARGET_LEDGER_CLOSE_SECONDS,
};
use crate::error::SentinelResult;
use crate::ports::reader::LedgerReader;

/// Request parameters for scanning a Soroban contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanRequest {
    /// Contract address encoded as Stellar Strkey (e.g. `CA3D...`).
    pub contract_id_strkey: String,
    /// List of persistent storage symbol keys to monitor (e.g. `["COUNTER", "ADMIN"]`).
    pub persistent_symbols: Vec<String>,
    /// List of persistent storage keys encoded as base64 `ScVal` XDR strings.
    pub persistent_xdr_keys: Vec<String>,
    /// Threshold configuration for warning and critical expiry evaluation.
    pub threshold_config: ThresholdConfig,
    /// Target network descriptor (e.g. "testnet", "mainnet", etc.).
    pub network: Option<String>,
}

impl ScanRequest {
    /// Create a new `ScanRequest` with default thresholds and empty persistent keys.
    pub fn new(contract_id_strkey: impl Into<String>) -> Self {
        Self {
            contract_id_strkey: contract_id_strkey.into(),
            persistent_symbols: Vec::new(),
            persistent_xdr_keys: Vec::new(),
            threshold_config: ThresholdConfig::default(),
            network: None,
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

    /// Set a custom threshold configuration.
    pub fn with_threshold_config(mut self, threshold_config: ThresholdConfig) -> Self {
        self.threshold_config = threshold_config;
        self
    }

    /// Set the network name.
    pub fn with_network(mut self, network: impl Into<String>) -> Self {
        self.network = Some(network.into());
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
    /// Entry type classification (ContractInstance, ContractCode, PersistentData).
    pub entry_type: EntryType,
    /// Durability tier (Instance, Code, Persistent).
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
        }
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

/// Application orchestrator for scanning Soroban smart contract storage TTL and expiration risk.
pub struct ScanContractUseCase<R: LedgerReader> {
    reader: R,
}

impl<R: LedgerReader> ScanContractUseCase<R> {
    /// Creates a new `ScanContractUseCase` with the given reader.
    pub fn new(reader: R) -> Self {
        Self { reader }
    }

    /// Returns a reference to the underlying reader.
    pub fn reader(&self) -> &R {
        &self.reader
    }

    /// Executes the full contract scan workflow.
    pub async fn execute(&self, request: ScanRequest) -> SentinelResult<TTLReport> {
        let contract_id_bytes = parse_contract_id(&request.contract_id_strkey)?;
        request.threshold_config.validate()?;
        let latest_ledger_info = self.reader.get_latest_ledger().await?;
        let latest_ledger = latest_ledger_info.sequence;

        let network = request
            .network
            .clone()
            .unwrap_or_else(|| "testnet".to_string());

        // 1. Derive deterministic ContractInstance key
        let instance_key_xdr = derive_contract_instance_key(&contract_id_bytes)?;

        // 2. Derive persistent keys from symbols and base64 ScVal
        let mut persistent_descriptors: Vec<(String, String)> = Vec::new();
        for sym in &request.persistent_symbols {
            let key_xdr = derive_persistent_symbol_key(&contract_id_bytes, sym)?;
            persistent_descriptors.push((format!("persistent:{}", sym), key_xdr));
        }
        for xdr_key in &request.persistent_xdr_keys {
            let key_xdr = derive_persistent_xdr_key(&contract_id_bytes, xdr_key)?;
            persistent_descriptors.push((format!("persistent_xdr:{}", xdr_key), key_xdr));
        }

        // 3. Query initial batch: instance key + persistent keys
        let mut query_keys: Vec<String> = Vec::with_capacity(1 + persistent_descriptors.len());
        query_keys.push(instance_key_xdr.clone());
        for (_, key_xdr) in &persistent_descriptors {
            query_keys.push(key_xdr.clone());
        }

        let raw_entries = self.reader.get_ledger_entries(&query_keys).await?;

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
            let remaining_seconds = estimate_remaining_seconds(remaining_ledgers);
            let remaining_days = estimate_remaining_days(remaining_seconds);
            let status = evaluate_risk(remaining_ledgers, &request.threshold_config);

            reports.push(StorageEntryReport::new(
                "instance",
                EntryType::ContractInstance,
                Durability::Instance,
                byte_size,
                live_until,
                remaining_ledgers,
                remaining_seconds,
                remaining_days,
                status,
            ));

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
                let remaining_seconds = estimate_remaining_seconds(remaining_ledgers);
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

        // 6. Process Persistent entries
        for (ident, key_xdr) in persistent_descriptors {
            let found_entry = raw_entries.iter().find(|e| e.key_xdr_base64 == key_xdr);

            if let Some(entry) = found_entry {
                let byte_size = compute_byte_size(&entry.entry_xdr_base64);
                let live_until = entry.live_until_ledger_seq;
                let remaining_ledgers = calculate_remaining_ledgers(live_until, latest_ledger);
                let remaining_seconds = estimate_remaining_seconds(remaining_ledgers);
                let remaining_days = estimate_remaining_days(remaining_seconds);
                let status = evaluate_risk(remaining_ledgers, &request.threshold_config);

                reports.push(StorageEntryReport::new(
                    ident,
                    EntryType::PersistentData,
                    Durability::Persistent,
                    byte_size,
                    live_until,
                    remaining_ledgers,
                    remaining_seconds,
                    remaining_days,
                    status,
                ));
            } else {
                // Missing/archived persistent entry
                reports.push(StorageEntryReport::new(
                    ident,
                    EntryType::PersistentData,
                    Durability::Persistent,
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
            ledger_velocity_seconds: TARGET_LEDGER_CLOSE_SECONDS,
            entries: reports,
            overall_status,
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
}
