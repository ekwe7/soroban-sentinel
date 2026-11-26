//! Use case orchestrator for scanning Soroban smart contract storage TTL and expiration risk.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use stellar_xdr::curr::{ReadXdr, WriteXdr};
use tracing::info;

use crate::adapters::{
    derive_contract_code_key, derive_contract_instance_key, derive_persistent_key_from_scval,
    derive_persistent_symbol_key, derive_persistent_xdr_key, extract_instance_storage_breakdown,
    extract_sac_asset_info, extract_wasm_hash_from_instance_entry, parse_contract_id,
    parse_key_expr, safe_xdr_limits,
};
use crate::domain::burn_rate::{calculate_burn_rate, BurnRate, DEFAULT_SAMPLE_WINDOW_LEDGERS};
use crate::domain::cold_storage::{
    recommend_archival, ArchivalPolicy, ArchivalRecommendation, ExtensionHistory,
};
use crate::domain::{
    calculate_remaining_ledgers, detect_contract_upgrade, estimate_remaining_days,
    estimate_remaining_seconds_with_velocity, evaluate_risk_for_durability,
    format_contract_code_hash, ContractUpgradeBaseline, ContractUpgradeEvent, Durability,
    EntryType, InstanceStorageBreakdown, RentDebt, RentDebtCalculator, RiskLevel, SacAssetInfo,
    StorageEntry, ThresholdConfig, TARGET_LEDGER_CLOSE_SECONDS,
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
    /// Threshold configuration for warning and critical expiry evaluation.
    pub threshold_config: ThresholdConfig,
    /// Target network descriptor (e.g. "testnet", "mainnet", etc.).
    pub network: Option<String>,
    /// Optional override for ledger close velocity in seconds per ledger.
    pub ledger_seconds_override: Option<f64>,
    /// Number of recent ledgers sampled when measuring burn velocity.
    #[serde(default = "default_sample_window")]
    pub sample_window: u32,
}

fn default_sample_window() -> u32 {
    DEFAULT_SAMPLE_WINDOW_LEDGERS
}

impl ScanRequest {
    /// Create a new `ScanRequest` with default thresholds and empty persistent keys.
    pub fn new(contract_id_strkey: impl Into<String>) -> Self {
        Self {
            contract_id_strkey: contract_id_strkey.into(),
            persistent_symbols: Vec::new(),
            persistent_xdr_keys: Vec::new(),
            persistent_key_exprs: Vec::new(),
            threshold_config: ThresholdConfig::default(),
            network: None,
            ledger_seconds_override: None,
            sample_window: DEFAULT_SAMPLE_WINDOW_LEDGERS,
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

    /// Set an optional ledger close velocity override.
    pub fn with_ledger_seconds(mut self, ledger_seconds: Option<f64>) -> Self {
        self.ledger_seconds_override = ledger_seconds;
        self
    }

    /// Set the number of recent ledgers sampled to measure burn velocity.
    pub fn with_sample_window(mut self, sample_window: u32) -> Self {
        self.sample_window = sample_window;
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
    /// Per-key breakdown of instance-scoped state inside `ScContractInstance.storage`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub instance_storage_keys: Vec<InstanceStorageBreakdown>,
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
            instance_storage_keys: Vec::new(),
        }
    }

    /// Sets the instance storage keys breakdown.
    pub fn with_instance_storage_keys(mut self, keys: Vec<InstanceStorageBreakdown>) -> Self {
        self.instance_storage_keys = keys;
        self
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
    /// Observed storage burn velocity and sampling metadata.
    #[serde(default)]
    pub burn_rate: BurnRate,
    /// Individual reports for all monitored storage entries.
    pub entries: Vec<StorageEntryReport>,
    /// Aggregate overall risk status for the contract.
    pub overall_status: RiskLevel,
    /// Advisory projections for eligible persistent entries.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub archival_recommendations: Vec<ArchivalRecommendation>,
    /// Extracted metadata if the contract is a Stellar Asset Contract (SAC).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sac_asset_info: Option<SacAssetInfo>,
    /// WASM code hash observed in the contract instance, when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contract_code_hash: Option<String>,
    /// WASM bytecode size from the contract's `ContractCodeEntry`, when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contract_bytecode_size: Option<u32>,
    /// Upgrade detected against the configured historical executable baseline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contract_upgrade: Option<ContractUpgradeEvent>,
}

impl TTLReport {
    /// Calculate per-entry restoration rent and modeled arrears using this scan's TTL observations.
    /// Entries absent from RPC need a known historical TTL and size supplied to the calculator directly.
    /// Results use the scan's measured or estimated entry sizes.
    pub fn rent_debts<'a>(
        &'a self,
        calculator: &'a RentDebtCalculator,
    ) -> impl Iterator<Item = (&'a StorageEntryReport, RentDebt)> + 'a {
        self.entries.iter().filter_map(move |entry| {
            let byte_size = u32::try_from(entry.byte_size).ok()?;
            let debt = calculator.calculate(
                entry.durability,
                byte_size,
                entry.live_until_ledger,
                self.latest_ledger,
            )?;
            Some((entry, debt))
        })
    }

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
#[tracing::instrument(skip(entry_xdr_base64), fields(xdr.base64_len = entry_xdr_base64.len()))]
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

fn contract_code_bytecode_size(entry_xdr_base64: &str) -> Option<u32> {
    let data = if let Ok(entry) =
        stellar_xdr::curr::LedgerEntry::from_xdr_base64(entry_xdr_base64, safe_xdr_limits())
    {
        entry.data
    } else {
        stellar_xdr::curr::LedgerEntryData::from_xdr_base64(entry_xdr_base64, safe_xdr_limits())
            .ok()?
    };
    let stellar_xdr::curr::LedgerEntryData::ContractCode(code) = data else {
        return None;
    };
    u32::try_from(code.code.len()).ok()
}

/// Fuzzing entry point for the scan byte-size computation.
///
/// Keeps the production helper private while allowing the fuzz target in `fuzz/`
/// to exercise it with arbitrary base64 XDR text.
#[cfg(feature = "fuzzing")]
#[doc(hidden)]
pub fn fuzz_compute_byte_size(entry_xdr_base64: &str) -> usize {
    compute_byte_size(entry_xdr_base64)
}

/// Collapse descriptors that resolve to the same canonical `LedgerKey` XDR.
///
/// Each key keeps the position of its first appearance. When an identifier
/// derived from a raw `persistent_xdr:` value collides with a readable
/// `persistent:` identifier, the readable one is kept.
fn dedup_persistent_descriptors(descriptors: Vec<(String, String)>) -> Vec<(String, String)> {
    let mut positions: HashMap<String, usize> = HashMap::with_capacity(descriptors.len());
    let mut unique: Vec<(String, String)> = Vec::with_capacity(descriptors.len());
    for (ident, key_xdr) in descriptors {
        match positions.get(&key_xdr) {
            Some(&index) => {
                let existing = &mut unique[index].0;
                if existing.starts_with("persistent_xdr:") && !ident.starts_with("persistent_xdr:")
                {
                    *existing = ident;
                }
            }
            None => {
                positions.insert(key_xdr.clone(), unique.len());
                unique.push((ident, key_xdr));
            }
        }
    }
    unique
}

/// Application orchestrator for scanning Soroban smart contract storage TTL and expiration risk.
pub struct ScanContractUseCase<R: LedgerReader> {
    reader: R,
    archival: Option<(ArchivalPolicy, HashMap<String, ExtensionHistory>)>,
    upgrade_baseline: Option<(ContractUpgradeBaseline, RentDebtCalculator)>,
}

impl<R: LedgerReader> ScanContractUseCase<R> {
    /// Creates a new `ScanContractUseCase` with the given reader.
    pub fn new(reader: R) -> Self {
        Self {
            reader,
            archival: None,
            upgrade_baseline: None,
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

    /// Compare WASM executable state with a prior scan and calculate the minimum-TTL rent delta.
    pub fn with_upgrade_baseline(
        mut self,
        baseline: ContractUpgradeBaseline,
        rent_calculator: RentDebtCalculator,
    ) -> Self {
        self.upgrade_baseline = Some((baseline, rent_calculator));
        self
    }

    /// Returns a reference to the underlying reader.
    pub fn reader(&self) -> &R {
        &self.reader
    }

    /// Executes the full contract scan workflow.
    #[tracing::instrument(
        skip(self, request),
        fields(
            contract_id = %request.contract_id_strkey,
            network = request.network.as_deref().unwrap_or("testnet"),
            entry.count = tracing::field::Empty,
            overall.status = tracing::field::Empty
        )
    )]
    pub async fn execute(&self, request: ScanRequest) -> SentinelResult<TTLReport> {
        self.execute_with_keys(request)
            .await
            .map(|(report, _)| report)
    }

    /// Returns raw ledger keys in the same order as the report entries.
    pub async fn execute_with_keys(
        &self,
        request: ScanRequest,
    ) -> SentinelResult<(TTLReport, Vec<String>)> {
        let contract_id_bytes = parse_contract_id(&request.contract_id_strkey)?;
        request.threshold_config.validate()?;
        let latest_ledger_info = self.reader.get_latest_ledger().await?;
        let latest_ledger = latest_ledger_info.sequence;

        let network = request
            .network
            .clone()
            .unwrap_or_else(|| "testnet".to_string());

        info!(contract_id = %request.contract_id_strkey, "scan_started");

        // 1. Derive deterministic ContractInstance key
        let instance_key_xdr = derive_contract_instance_key(&contract_id_bytes)?;
        info!(?instance_key_xdr, "instance_key_derived");

        // Resolve storage burn velocity: explicit CLI override, historical
        // ledger close samples, a legacy single velocity measurement, or the
        // standard 5.0s target when RPC history is pruned or unavailable.
        let burn_rate = if let Some(override_seconds) = request.ledger_seconds_override {
            BurnRate::measured(override_seconds)
        } else {
            match self
                .reader
                .get_ledger_close_samples(latest_ledger, request.sample_window)
                .await
            {
                Ok(Some(samples)) => calculate_burn_rate(&samples, TARGET_LEDGER_CLOSE_SECONDS),
                _ => match self.reader.get_ledger_close_velocity(latest_ledger).await {
                    Ok(Some(velocity)) => BurnRate::measured(velocity),
                    _ => BurnRate::fallback(),
                },
            }
        };
        let ledger_velocity = burn_rate.seconds_per_ledger;
        info!(ledger_velocity = %ledger_velocity, "ledger_velocity_resolved");

        // 2. Derive persistent keys from symbols, base64 ScVal, and typed key expressions
        let mut persistent_descriptors: Vec<(String, String)> = Vec::new();
        for sym in &request.persistent_symbols {
            let key_xdr = derive_persistent_symbol_key(&contract_id_bytes, sym)?;
            persistent_descriptors.push((format!("persistent:{}", sym), key_xdr));
        }
        for xdr_key in &request.persistent_xdr_keys {
            let key_xdr = derive_persistent_xdr_key(&contract_id_bytes, xdr_key)?;
            persistent_descriptors.push((format!("persistent_xdr:{}", xdr_key), key_xdr));
        }
        for expr in &request.persistent_key_exprs {
            let scval = parse_key_expr(expr)?;
            let key_xdr = derive_persistent_key_from_scval(&contract_id_bytes, scval)?;
            persistent_descriptors.push((format!("persistent:{}", expr), key_xdr));
        }
        let persistent_descriptors = dedup_persistent_descriptors(persistent_descriptors);
        info!(
            persistent_key_count = persistent_descriptors.len(),
            "persistent_keys_derived"
        );

        // 3. Query initial batch: instance key + persistent keys
        let mut query_keys: Vec<String> = Vec::with_capacity(1 + persistent_descriptors.len());
        query_keys.push(instance_key_xdr.clone());
        for (_, key_xdr) in &persistent_descriptors {
            query_keys.push(key_xdr.clone());
        }

        let raw_entries = self.reader.get_ledger_entries(&query_keys).await?;
        info!(
            ?query_keys,
            entry_count = raw_entries.len(),
            "ledger_entries_queried"
        );

        let mut reports = Vec::new();

        // 4. Process ContractInstance entry
        let instance_entry_opt = raw_entries
            .iter()
            .find(|e| e.key_xdr_base64 == instance_key_xdr);

        let mut wasm_hash_opt: Option<[u8; 32]> = None;
        let mut sac_asset_info_opt: Option<SacAssetInfo> = None;
        let mut contract_bytecode_size = None;
        let mut contract_upgrade = None;

        if let Some(entry) = instance_entry_opt {
            let byte_size = compute_byte_size(&entry.entry_xdr_base64);
            let live_until = entry.live_until_ledger_seq;
            let remaining_ledgers = calculate_remaining_ledgers(live_until, latest_ledger);
            let remaining_seconds =
                estimate_remaining_seconds_with_velocity(remaining_ledgers, ledger_velocity);
            let remaining_days = estimate_remaining_days(remaining_seconds);
            let status = evaluate_risk_for_durability(
                remaining_ledgers,
                Durability::Instance,
                &request.threshold_config,
            );

            let mut instance_report = StorageEntryReport::new(
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

            // Extract breakdown of embedded storage map keys
            if let Ok(keys_breakdown) = extract_instance_storage_breakdown(&entry.entry_xdr_base64)
            {
                instance_report.instance_storage_keys = keys_breakdown;
            }

            reports.push(instance_report);

            // Extract SAC AssetInfo if contract is a Stellar Asset Contract
            sac_asset_info_opt = extract_sac_asset_info(&entry.entry_xdr_base64)
                .ok()
                .flatten();

            // Extract WASM hash if executable is WASM bytecode
            wasm_hash_opt = extract_wasm_hash_from_instance_entry(&entry.entry_xdr_base64)
                .ok()
                .flatten();
            let wasm_hash_str = wasm_hash_opt
                .map(|h| format!("{:?}", h))
                .unwrap_or_else(|| "none".to_string());
            info!(
                "instance_processed: wasm_hash={} byte_size={} status={}",
                wasm_hash_str, byte_size, status
            );
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
            info!("instance_entry_missing, status=archived");
        }

        let mut report_keys = vec![instance_key_xdr.clone()];

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

            report_keys.push(code_key_xdr);
            if let Some(code_entry) = code_entry_opt {
                let bytecode_size = contract_code_bytecode_size(&code_entry.entry_xdr_base64);
                if let Some(size) = bytecode_size {
                    contract_bytecode_size = Some(size);
                    if let (Some((baseline, calculator)), Some(current_hash)) =
                        (&self.upgrade_baseline, wasm_hash_opt)
                    {
                        let baseline_rent =
                            calculator.minimum_ttl_rent_stroops(baseline.bytecode_size);
                        let current_rent = calculator.minimum_ttl_rent_stroops(size);
                        let baseline_rent = i128::try_from(baseline_rent).map_err(|_| {
                            crate::error::SentinelError::SerializationError(
                                "baseline minimum-TTL rent exceeds supported range".to_string(),
                            )
                        })?;
                        let current_rent = i128::try_from(current_rent).map_err(|_| {
                            crate::error::SentinelError::SerializationError(
                                "current minimum-TTL rent exceeds supported range".to_string(),
                            )
                        })?;
                        let rent_delta =
                            current_rent.checked_sub(baseline_rent).ok_or_else(|| {
                                crate::error::SentinelError::SerializationError(
                                    "minimum-TTL rent delta exceeds supported range".to_string(),
                                )
                            })?;
                        contract_upgrade = detect_contract_upgrade(
                            *baseline,
                            current_hash,
                            size,
                            latest_ledger,
                            rent_delta,
                        );
                    }
                } else if self.upgrade_baseline.is_some() {
                    return Err(crate::error::SentinelError::SerializationError(
                        "contract code entry could not be decoded for upgrade tracking".into(),
                    ));
                }
                let byte_size = compute_byte_size(&code_entry.entry_xdr_base64);
                let live_until = code_entry.live_until_ledger_seq;
                let remaining_ledgers = calculate_remaining_ledgers(live_until, latest_ledger);
                let remaining_seconds =
                    estimate_remaining_seconds_with_velocity(remaining_ledgers, ledger_velocity);
                let remaining_days = estimate_remaining_days(remaining_seconds);
                let status = evaluate_risk_for_durability(
                    remaining_ledgers,
                    Durability::Code,
                    &request.threshold_config,
                );

                let code_entry_str = code_entry.key_xdr_base64.clone();
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
                info!(
                    "code_processed: code_entry={} byte_size={} status={}",
                    code_entry_str, byte_size, status
                );
            } else {
                if self.upgrade_baseline.is_some() {
                    return Err(crate::error::SentinelError::SerializationError(
                        "contract code entry is missing during upgrade tracking".into(),
                    ));
                }
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
                info!("code_entry_missing, status=archived");
            }
        }

        let mut archival_recommendations = Vec::new();
        let mut evaluated_keys = HashSet::new();

        // 6. Process Persistent entries
        let persistent_count = persistent_descriptors.len();
        for (ident, key_xdr) in persistent_descriptors {
            let found_entry = raw_entries.iter().find(|e| e.key_xdr_base64 == key_xdr);
            report_keys.push(key_xdr.clone());

            if let Some(entry) = found_entry {
                let byte_size = compute_byte_size(&entry.entry_xdr_base64);
                let live_until = entry.live_until_ledger_seq;
                let remaining_ledgers = calculate_remaining_ledgers(live_until, latest_ledger);
                let remaining_seconds =
                    estimate_remaining_seconds_with_velocity(remaining_ledgers, ledger_velocity);
                let remaining_days = estimate_remaining_days(remaining_seconds);
                let status = evaluate_risk_for_durability(
                    remaining_ledgers,
                    Durability::Persistent,
                    &request.threshold_config,
                );

                if let Some((policy, histories)) = &self.archival {
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
        info!(
            persistent_processed = persistent_count,
            "persistent_entries_processed"
        );

        let overall_status = compute_overall_status(&reports);
        let span = tracing::Span::current();
        span.record("entry.count", reports.len() as u64);
        span.record("overall.status", tracing::field::display(overall_status));

        info!(
            overall_status = %overall_status,
            total_entries = reports.len(),
        );

        Ok((
            TTLReport {
                contract_id: request.contract_id_strkey,
                network,
                latest_ledger,
                ledger_velocity_seconds: ledger_velocity,
                burn_rate,
                entries: reports,
                overall_status,
                archival_recommendations,
                sac_asset_info: sac_asset_info_opt,
                contract_code_hash: wasm_hash_opt.map(|hash| format_contract_code_hash(&hash)),
                contract_bytecode_size,
                contract_upgrade,
            },
            report_keys,
        ))
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

    use crate::domain::burn_rate::{BurnRateSource, LedgerCloseSample};
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

    #[cfg(feature = "fuzzing")]
    #[test]
    fn test_fuzz_compute_byte_size_handles_malformed_input() {
        assert_eq!(fuzz_compute_byte_size(""), 0);
        assert_eq!(fuzz_compute_byte_size("!!!"), 2);
        assert!(fuzz_compute_byte_size("not-valid-xdr-base64!!") > 0);
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

    #[tokio::test]
    async fn test_scan_with_historical_samples_uses_measured_burn_rate() {
        let contract_bytes = [60u8; 32];
        let contract_strkey = stellar_strkey::Contract(contract_bytes).to_string();

        let instance_key = derive_contract_instance_key(&contract_bytes).unwrap();
        let instance_xdr = make_test_instance_xdr(contract_bytes, [61u8; 32]);
        let instance_raw = RawLedgerEntry::new(&instance_key, &instance_xdr, 5_500_000, 5_400_000);

        // Ledgers 100, 200 and 300 closed after 1000, 1550 and 2100 seconds:
        // an exactly 5.5s/ledger linear trend.
        let samples = vec![
            LedgerCloseSample::new(100, 1_000),
            LedgerCloseSample::new(200, 1_550),
            LedgerCloseSample::new(300, 2_100),
        ];
        let mock_reader = MockLedgerReader::new()
            .with_latest_ledger(5_400_000, 22)
            .with_close_samples(samples)
            .with_entry(instance_raw);

        let usecase = ScanContractUseCase::new(mock_reader);
        let request = ScanRequest::new(&contract_strkey).with_sample_window(250);

        let report = usecase.execute(request).await.unwrap();

        assert_eq!(report.ledger_velocity_seconds, 5.5);
        assert_eq!(report.burn_rate.source, BurnRateSource::Measured);
        assert_eq!(report.burn_rate.sample_count, 3);
        assert_eq!(report.burn_rate.observed_ledgers, 200);
        assert_eq!(report.burn_rate.latest_close_time, Some(2_100));

        let inst = &report.entries[0];
        assert_eq!(inst.estimated_remaining_seconds, 550_000.0);
    }

    #[tokio::test]
    async fn test_scan_stellar_asset_contract_extracts_sac_metadata() {
        let contract_bytes = [60u8; 32];
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

        assert!(report.sac_asset_info.is_some());
        let info = report.sac_asset_info.unwrap();
        assert!(info.is_native);
        assert_eq!(info.symbol, Some("XLM".to_string()));
    }

    #[tokio::test]
    async fn test_scan_instance_storage_breakdown_surface() {
        use stellar_xdr::curr::{ScMap, ScMapEntry};

        let contract_bytes = [70u8; 32];
        let contract_strkey = stellar_strkey::Contract(contract_bytes).to_string();

        let instance_key = derive_contract_instance_key(&contract_bytes).unwrap();
        let storage_map = ScMap(
            vec![ScMapEntry {
                key: ScVal::Symbol(ScSymbol("AdminKey".try_into().unwrap())),
                val: ScVal::U32(1),
            }]
            .try_into()
            .unwrap(),
        );

        let entry_data = LedgerEntryData::ContractData(ContractDataEntry {
            ext: ExtensionPoint::V0,
            contract: ScAddress::Contract(Hash(contract_bytes)),
            key: ScVal::LedgerKeyContractInstance,
            durability: ContractDataDurability::Persistent,
            val: ScVal::ContractInstance(ScContractInstance {
                executable: ContractExecutable::Wasm(Hash([71u8; 32])),
                storage: Some(storage_map),
            }),
        });
        let instance_xdr = entry_data.to_xdr_base64(Limits::none()).unwrap();

        let code_key = derive_contract_code_key(&[71u8; 32]).unwrap();
        let code_entry_data = LedgerEntryData::ContractCode(ContractCodeEntry {
            ext: ContractCodeEntryExt::V0,
            hash: Hash([71u8; 32]),
            code: vec![0u8; 64].try_into().unwrap(),
        });
        let code_xdr = code_entry_data.to_xdr_base64(Limits::none()).unwrap();

        let instance_raw = RawLedgerEntry::new(&instance_key, &instance_xdr, 6_000_000, 5_400_000);
        let code_raw = RawLedgerEntry::new(&code_key, &code_xdr, 6_000_000, 5_400_000);

        let mock_reader = MockLedgerReader::new()
            .with_latest_ledger(5_400_000, 22)
            .with_entry(instance_raw)
            .with_entry(code_raw);

        let usecase = ScanContractUseCase::new(mock_reader);
        let request = ScanRequest::new(&contract_strkey);

        let report = usecase.execute(request).await.unwrap();

        let instance_rep = report
            .entries
            .iter()
            .find(|e| e.key_identifier == "instance")
            .unwrap();
        assert_eq!(instance_rep.instance_storage_keys.len(), 1);
        assert_eq!(
            instance_rep.instance_storage_keys[0].key_summary,
            "AdminKey"
        );

        let code_rep = report
            .entries
            .iter()
            .find(|e| e.key_identifier == "code_wasm")
            .unwrap();
        assert!(code_rep.instance_storage_keys.is_empty());
    }

    /// Delegates to `MockLedgerReader` while recording every `get_ledger_entries` batch.
    #[derive(Clone, Default)]
    struct RecordingReader {
        inner: MockLedgerReader,
        queries: std::sync::Arc<std::sync::Mutex<Vec<Vec<String>>>>,
    }

    #[async_trait::async_trait]
    impl LedgerReader for RecordingReader {
        async fn get_latest_ledger(
            &self,
        ) -> SentinelResult<crate::ports::reader::LatestLedgerInfo> {
            self.inner.get_latest_ledger().await
        }

        async fn get_ledger_entries(
            &self,
            keys_xdr_base64: &[String],
        ) -> SentinelResult<Vec<RawLedgerEntry>> {
            self.queries.lock().unwrap().push(keys_xdr_base64.to_vec());
            self.inner.get_ledger_entries(keys_xdr_base64).await
        }
    }

    /// Reader with a live instance, its code entry, and persistent `COUNTER`/`ADMIN` entries.
    fn dedup_fixture(contract_bytes: [u8; 32]) -> RecordingReader {
        let wasm_bytes = [contract_bytes[0].wrapping_add(1); 32];
        let instance_key = derive_contract_instance_key(&contract_bytes).unwrap();
        let code_key = derive_contract_code_key(&wasm_bytes).unwrap();
        let mut inner = MockLedgerReader::new()
            .with_latest_ledger(5_400_000, 22)
            .with_entry(RawLedgerEntry::new(
                &instance_key,
                make_test_instance_xdr(contract_bytes, wasm_bytes),
                5_800_000,
                5_390_000,
            ))
            .with_entry(RawLedgerEntry::new(
                &code_key,
                make_test_code_xdr(wasm_bytes, &[1, 2, 3]),
                5_800_000,
                5_390_000,
            ));
        for symbol in ["COUNTER", "ADMIN"] {
            inner = inner.with_entry(RawLedgerEntry::new(
                derive_persistent_symbol_key(&contract_bytes, symbol).unwrap(),
                make_test_persistent_symbol_xdr(contract_bytes, symbol, 1),
                5_800_000,
                5_390_000,
            ));
        }
        RecordingReader {
            inner,
            ..RecordingReader::default()
        }
    }

    fn symbol_scval_xdr(symbol: &str) -> String {
        ScVal::Symbol(ScSymbol(symbol.try_into().unwrap()))
            .to_xdr_base64(Limits::none())
            .unwrap()
    }

    fn identifiers(report: &TTLReport) -> Vec<&str> {
        report
            .entries
            .iter()
            .map(|e| e.key_identifier.as_str())
            .collect()
    }

    #[tokio::test]
    async fn test_scan_deduplicates_repeated_symbol_keys() {
        let contract_bytes = [60u8; 32];
        let reader = dedup_fixture(contract_bytes);
        let queries = reader.queries.clone();
        let usecase = ScanContractUseCase::new(reader);
        let request = ScanRequest::new(stellar_strkey::Contract(contract_bytes).to_string())
            .with_persistent_symbols(vec![
                "COUNTER".into(),
                "COUNTER".into(),
                "ADMIN".into(),
                "COUNTER".into(),
            ]);

        let (report, keys) = usecase.execute_with_keys(request).await.unwrap();

        assert_eq!(
            identifiers(&report),
            [
                "instance",
                "code_wasm",
                "persistent:COUNTER",
                "persistent:ADMIN"
            ]
        );
        let initial_batch = &queries.lock().unwrap()[0];
        assert_eq!(
            initial_batch,
            &vec![
                derive_contract_instance_key(&contract_bytes).unwrap(),
                derive_persistent_symbol_key(&contract_bytes, "COUNTER").unwrap(),
                derive_persistent_symbol_key(&contract_bytes, "ADMIN").unwrap(),
            ]
        );
        assert_eq!(keys.len(), report.entries.len());
    }

    #[tokio::test]
    async fn test_scan_prefers_symbol_identifier_over_equivalent_xdr_key() {
        let contract_bytes = [61u8; 32];
        let reader = dedup_fixture(contract_bytes);
        let queries = reader.queries.clone();
        let usecase = ScanContractUseCase::new(reader);
        let request = ScanRequest::new(stellar_strkey::Contract(contract_bytes).to_string())
            .with_persistent_symbols(vec!["COUNTER".into()])
            .with_persistent_xdr_keys(vec![symbol_scval_xdr("COUNTER")]);

        let report = usecase.execute(request).await.unwrap();

        assert_eq!(
            identifiers(&report),
            ["instance", "code_wasm", "persistent:COUNTER"]
        );
        assert_eq!(queries.lock().unwrap()[0].len(), 2);
    }

    #[tokio::test]
    async fn test_scan_readable_identifier_replaces_earlier_xdr_in_first_position() {
        let contract_bytes = [62u8; 32];
        let usecase = ScanContractUseCase::new(dedup_fixture(contract_bytes));
        // XDR keys are derived before key expressions, so the raw COUNTER key
        // appears first and the later readable expression must replace its
        // identifier without moving it.
        let request = ScanRequest::new(stellar_strkey::Contract(contract_bytes).to_string())
            .with_persistent_xdr_keys(vec![symbol_scval_xdr("COUNTER"), symbol_scval_xdr("ADMIN")])
            .with_persistent_key_exprs(vec!["sym:COUNTER".into()]);

        let report = usecase.execute(request).await.unwrap();

        assert_eq!(
            identifiers(&report),
            [
                "instance",
                "code_wasm",
                "persistent:sym:COUNTER",
                &format!("persistent_xdr:{}", symbol_scval_xdr("ADMIN")),
            ]
        );
    }

    #[tokio::test]
    async fn test_scan_summary_counts_each_deduplicated_key_once() {
        let contract_bytes = [63u8; 32];
        let usecase = ScanContractUseCase::new(dedup_fixture(contract_bytes));
        let request = ScanRequest::new(stellar_strkey::Contract(contract_bytes).to_string())
            .with_persistent_symbols(vec!["COUNTER".into(), "MISSING".into(), "MISSING".into()])
            .with_persistent_xdr_keys(vec![
                symbol_scval_xdr("COUNTER"),
                symbol_scval_xdr("MISSING"),
            ]);

        let report = usecase.execute(request).await.unwrap();

        assert_eq!(
            report.summary(),
            ReportSummary {
                total_entries: 4,
                ok: 3,
                warn: 0,
                critical: 0,
                archived: 1,
            }
        );
        assert_eq!(report.overall_status, RiskLevel::Archived);
    }

    #[tokio::test]
    async fn test_scan_applies_per_durability_threshold_overrides() {
        use crate::domain::ThresholdOverride;

        let contract_bytes = [80u8; 32];
        let wasm_bytes = [81u8; 32];
        let contract_strkey = stellar_strkey::Contract(contract_bytes).to_string();

        let instance_key = derive_contract_instance_key(&contract_bytes).unwrap();
        let code_key = derive_contract_code_key(&wasm_bytes).unwrap();
        let symbol_key = derive_persistent_symbol_key(&contract_bytes, "COUNTER").unwrap();

        let instance_xdr = make_test_instance_xdr(contract_bytes, wasm_bytes);
        let code_xdr = make_test_code_xdr(wasm_bytes, &[0xCA, 0xFE, 0xBA, 0xBE]);
        let symbol_xdr = make_test_persistent_symbol_xdr(contract_bytes, "COUNTER", 7);

        let latest_ledger = 5_400_000;
        // All three entries have identical remaining TTL: 150,000 ledgers.
        let live_until = latest_ledger + 150_000;
        let instance_raw = RawLedgerEntry::new(&instance_key, &instance_xdr, live_until, 5_390_000);
        let code_raw = RawLedgerEntry::new(&code_key, &code_xdr, live_until, 5_380_000);
        let symbol_raw = RawLedgerEntry::new(&symbol_key, &symbol_xdr, live_until, 5_370_000);

        let mock_reader = MockLedgerReader::new()
            .with_latest_ledger(latest_ledger, 22)
            .with_entry(instance_raw)
            .with_entry(code_raw)
            .with_entry(symbol_raw);

        // Global fallback: warn < 100k, crit < 20k (persistent falls back -> Ok)
        // Instance override: warn < 300k, crit < 100k -> Warn
        // Code override: warn < 500k, crit < 200k -> Critical
        let thresholds = ThresholdConfig::new(100_000, 20_000)
            .with_instance_override(ThresholdOverride::new(300_000, 100_000))
            .with_code_override(ThresholdOverride::new(500_000, 200_000));

        let usecase = ScanContractUseCase::new(mock_reader);
        let request = ScanRequest::new(&contract_strkey)
            .with_persistent_symbols(vec!["COUNTER".to_string()])
            .with_threshold_config(thresholds);

        let report = usecase.execute(request).await.expect("Scan should succeed");

        assert_eq!(report.entries.len(), 3);
        assert_eq!(report.entries[0].key_identifier, "instance");
        assert_eq!(report.entries[0].status, RiskLevel::Warn);
        assert_eq!(report.entries[1].key_identifier, "code_wasm");
        assert_eq!(report.entries[1].status, RiskLevel::Critical);
        assert_eq!(report.entries[2].key_identifier, "persistent:COUNTER");
        assert_eq!(report.entries[2].status, RiskLevel::Ok);
        assert_eq!(report.overall_status, RiskLevel::Critical);
    }

    #[tokio::test]
    async fn test_scan_rejects_invalid_per_durability_override() {
        use crate::domain::ThresholdOverride;
        use crate::error::SentinelError;

        let contract_bytes = [82u8; 32];
        let contract_strkey = stellar_strkey::Contract(contract_bytes).to_string();

        let mock_reader = MockLedgerReader::new().with_latest_ledger(5_400_000, 22);
        let usecase = ScanContractUseCase::new(mock_reader);

        let invalid_thresholds =
            ThresholdConfig::default().with_code_override(ThresholdOverride::new(50, 100));
        let request = ScanRequest::new(&contract_strkey).with_threshold_config(invalid_thresholds);

        let err = usecase.execute(request).await.unwrap_err();
        assert!(matches!(err, SentinelError::InvalidThresholdConfig(_)));
    }
}
