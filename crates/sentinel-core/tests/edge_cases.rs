//! Comprehensive edge-case coverage and boundary testing for `sentinel-core`.
//!
//! Validates failure modes, boundary thresholds, arithmetic underflow/overflow prevention,
//! and uninitialized/missing storage conditions adhering to `docs/TESTING_STRATEGY.md`.

use stellar_xdr::curr::{
    ContractDataDurability, ContractDataEntry, ContractExecutable, ExtensionPoint, Hash,
    LedgerEntryData, Limits, ScAddress, ScContractInstance, ScVal, WriteXdr,
};

use sentinel_core::adapters::{
    derive_contract_instance_key, derive_persistent_xdr_key, parse_contract_id,
};
use sentinel_core::domain::{
    calculate_remaining_ledgers, estimate_horizon_days, estimate_remaining_days,
    estimate_remaining_seconds, evaluate_risk, Durability, EntryType, RiskLevel, ThresholdConfig,
};
use sentinel_core::error::SentinelError;
use sentinel_core::ports::mock::MockLedgerReader;
use sentinel_core::ports::reader::RawLedgerEntry;
use sentinel_core::usecases::{ScanContractUseCase, ScanRequest};

fn make_instance_entry_xdr(contract_id: [u8; 32], executable: ContractExecutable) -> String {
    let entry_data = LedgerEntryData::ContractData(ContractDataEntry {
        ext: ExtensionPoint::V0,
        contract: ScAddress::Contract(Hash(contract_id)),
        key: ScVal::LedgerKeyContractInstance,
        durability: ContractDataDurability::Persistent,
        val: ScVal::ContractInstance(ScContractInstance {
            executable,
            storage: None,
        }),
    });
    entry_data.to_xdr_base64(Limits::none()).unwrap()
}

// ---------------------------------------------------------------------------
// 1. Contract instance without WASM code executable (e.g. built-in Stellar asset contract)
// ---------------------------------------------------------------------------
#[tokio::test]
async fn test_edge_case_contract_instance_without_wasm_executable() {
    let contract_id = [11u8; 32];
    let contract_strkey = stellar_strkey::Contract(contract_id).to_string();

    let instance_key = derive_contract_instance_key(&contract_id).unwrap();
    let instance_xdr = make_instance_entry_xdr(contract_id, ContractExecutable::StellarAsset);

    // Live until 6,000,000, current = 5,400,000 -> 600,000 remaining -> OK
    let instance_raw = RawLedgerEntry::new(&instance_key, &instance_xdr, 6_000_000, 5_390_000);

    let mock_reader = MockLedgerReader::new()
        .with_latest_ledger(5_400_000, 22)
        .with_entry(instance_raw);

    let usecase = ScanContractUseCase::new(mock_reader);
    let request = ScanRequest::new(&contract_strkey);

    let report = usecase
        .execute(request)
        .await
        .expect("Scan on built-in asset contract must succeed without error");

    // Exactly 1 entry: instance only, NO code_wasm entry attempted
    assert_eq!(report.entries.len(), 1);
    assert_eq!(report.entries[0].key_identifier, "instance");
    assert_eq!(report.entries[0].entry_type, EntryType::ContractInstance);
    assert_eq!(report.entries[0].durability, Durability::Instance);
    assert_eq!(report.entries[0].live_until_ledger, 6_000_000);
    assert_eq!(report.entries[0].remaining_ledgers, 600_000);
    assert_eq!(report.entries[0].status, RiskLevel::Ok);
    assert_eq!(report.overall_status, RiskLevel::Ok);
}

// ---------------------------------------------------------------------------
// 2. Contract instance already archived (liveUntilLedgerSeq == 0 or expired)
// ---------------------------------------------------------------------------
#[tokio::test]
async fn test_edge_case_contract_instance_already_archived_zero_live_until() {
    let contract_id = [12u8; 32];
    let contract_strkey = stellar_strkey::Contract(contract_id).to_string();

    let instance_key = derive_contract_instance_key(&contract_id).unwrap();
    let instance_xdr =
        make_instance_entry_xdr(contract_id, ContractExecutable::Wasm(Hash([99u8; 32])));

    // Explicitly live_until == 0 (archived indicator)
    let instance_raw = RawLedgerEntry::new(&instance_key, &instance_xdr, 0, 5_000_000);

    let mock_reader = MockLedgerReader::new()
        .with_latest_ledger(5_400_000, 22)
        .with_entry(instance_raw);

    let usecase = ScanContractUseCase::new(mock_reader);
    let request = ScanRequest::new(&contract_strkey);

    let report = usecase
        .execute(request)
        .await
        .expect("Scan on archived instance must succeed without panicking");

    let inst = &report.entries[0];
    assert_eq!(inst.key_identifier, "instance");
    assert_eq!(inst.live_until_ledger, 0);
    assert_eq!(inst.remaining_ledgers, 0);
    assert_eq!(inst.estimated_remaining_seconds, 0.0);
    assert_eq!(inst.remaining_days, 0.0);
    assert_eq!(inst.status, RiskLevel::Archived);
    assert_eq!(report.overall_status, RiskLevel::Archived);
}

#[tokio::test]
async fn test_edge_case_contract_instance_expired_in_past_ledger() {
    let contract_id = [13u8; 32];
    let contract_strkey = stellar_strkey::Contract(contract_id).to_string();

    let instance_key = derive_contract_instance_key(&contract_id).unwrap();
    let instance_xdr =
        make_instance_entry_xdr(contract_id, ContractExecutable::Wasm(Hash([98u8; 32])));

    // Expired in past: live_until = 5,399,900, latest = 5,400,000 -> -100 ledgers
    let instance_raw = RawLedgerEntry::new(&instance_key, &instance_xdr, 5_399_900, 5_390_000);

    let mock_reader = MockLedgerReader::new()
        .with_latest_ledger(5_400_000, 22)
        .with_entry(instance_raw);

    let usecase = ScanContractUseCase::new(mock_reader);
    let request = ScanRequest::new(&contract_strkey);

    let report = usecase
        .execute(request)
        .await
        .expect("Scan on past-expired contract must not panic");

    let inst = &report.entries[0];
    assert_eq!(inst.remaining_ledgers, -100);
    assert_eq!(inst.estimated_remaining_seconds, 0.0);
    assert_eq!(inst.remaining_days, 0.0);
    assert_eq!(inst.status, RiskLevel::Archived);
    assert_eq!(report.overall_status, RiskLevel::Archived);
}

// ---------------------------------------------------------------------------
// 3. Requested persistent key missing from response (never initialized or pruned)
// ---------------------------------------------------------------------------
#[tokio::test]
async fn test_edge_case_requested_persistent_keys_missing_from_storage() {
    let contract_id = [14u8; 32];
    let contract_strkey = stellar_strkey::Contract(contract_id).to_string();

    let instance_key = derive_contract_instance_key(&contract_id).unwrap();
    let instance_xdr = make_instance_entry_xdr(contract_id, ContractExecutable::StellarAsset);
    let instance_raw = RawLedgerEntry::new(&instance_key, &instance_xdr, 5_600_000, 5_390_000);

    // Reader only returns the instance entry. Persistent keys requested are NOT in reader.
    let mock_reader = MockLedgerReader::new()
        .with_latest_ledger(5_400_000, 22)
        .with_entry(instance_raw);

    let scval_key = ScVal::U32(777).to_xdr_base64(Limits::none()).unwrap();

    let usecase = ScanContractUseCase::new(mock_reader);
    let request = ScanRequest::new(&contract_strkey)
        .with_persistent_symbols(vec!["UNINITIALIZED_SYM".to_string()])
        .with_persistent_xdr_keys(vec![scval_key.clone()]);

    let report = usecase
        .execute(request)
        .await
        .expect("Scan with missing persistent keys must succeed gracefully");

    assert_eq!(report.entries.len(), 3);

    // Instance entry is OK
    assert_eq!(report.entries[0].key_identifier, "instance");
    assert_eq!(report.entries[0].status, RiskLevel::Ok);

    // Missing symbol entry is ARCHIVED
    assert_eq!(
        report.entries[1].key_identifier,
        "persistent:UNINITIALIZED_SYM"
    );
    assert_eq!(report.entries[1].entry_type, EntryType::PersistentData);
    assert_eq!(report.entries[1].durability, Durability::Persistent);
    assert_eq!(report.entries[1].byte_size, 0);
    assert_eq!(report.entries[1].live_until_ledger, 0);
    assert_eq!(report.entries[1].status, RiskLevel::Archived);

    // Missing XDR key entry is ARCHIVED
    assert_eq!(
        report.entries[2].key_identifier,
        format!("persistent_xdr:{}", scval_key)
    );
    assert_eq!(report.entries[2].status, RiskLevel::Archived);

    // Overall status must escalate to ARCHIVED due to missing persistent keys
    assert_eq!(report.overall_status, RiskLevel::Archived);
}

// ---------------------------------------------------------------------------
// 4. Extremely large remaining ledger horizon (> 10 years / near u32::MAX)
// ---------------------------------------------------------------------------
#[tokio::test]
async fn test_edge_case_extremely_large_ledger_horizon() {
    let contract_id = [15u8; 32];
    let contract_strkey = stellar_strkey::Contract(contract_id).to_string();

    let instance_key = derive_contract_instance_key(&contract_id).unwrap();
    let instance_xdr = make_instance_entry_xdr(contract_id, ContractExecutable::StellarAsset);

    // 100,000,000 ledgers remaining (~15.8 years at 5s/ledger)
    let live_until = 105_400_000;
    let latest_ledger = 5_400_000;
    let instance_raw = RawLedgerEntry::new(&instance_key, &instance_xdr, live_until, 5_390_000);

    let mock_reader = MockLedgerReader::new()
        .with_latest_ledger(latest_ledger, 22)
        .with_entry(instance_raw);

    let usecase = ScanContractUseCase::new(mock_reader);
    let request = ScanRequest::new(&contract_strkey);

    let report = usecase
        .execute(request)
        .await
        .expect("Scan with large horizon must not overflow");

    let inst = &report.entries[0];
    assert_eq!(inst.remaining_ledgers, 100_000_000);
    assert_eq!(inst.estimated_remaining_seconds, 500_000_000.0);

    // 500,000,000 / 86400 ~= 5787.037 days (> 15 years)
    assert!(inst.remaining_days > 5_780.0);
    assert_eq!(inst.status, RiskLevel::Ok);
    assert_eq!(report.overall_status, RiskLevel::Ok);
}

#[test]
fn test_edge_case_near_u32_max_arithmetic_overflow_prevention() {
    let live_until = u32::MAX; // 4,294,967,295
    let latest_ledger = 5_000_000;

    let remaining_ledgers = calculate_remaining_ledgers(live_until, latest_ledger);
    assert_eq!(remaining_ledgers, 4_289_967_295);

    let remaining_seconds = estimate_remaining_seconds(remaining_ledgers);
    assert_eq!(remaining_seconds, 4_289_967_295.0 * 5.0);

    let remaining_days = estimate_remaining_days(remaining_seconds);
    assert!(remaining_days > 248_000.0); // > 679 years

    let horizon = estimate_horizon_days(live_until, latest_ledger);
    assert_eq!(remaining_days, horizon);

    let config = ThresholdConfig::default();
    assert_eq!(evaluate_risk(remaining_ledgers, &config), RiskLevel::Ok);
}

// ---------------------------------------------------------------------------
// 5. Boundary testing for Risk Classification Engine
// ---------------------------------------------------------------------------
#[test]
fn test_edge_case_exact_threshold_boundaries() {
    let config = ThresholdConfig::new(120_960, 34_560);

    // Boundary: Negative ledgers -> Archived
    assert_eq!(evaluate_risk(-1, &config), RiskLevel::Archived);
    assert_eq!(evaluate_risk(i64::MIN, &config), RiskLevel::Archived);

    // Boundary: Exactly 0 -> Archived
    assert_eq!(evaluate_risk(0, &config), RiskLevel::Archived);

    // Boundary: 1 ledger remaining -> Critical
    assert_eq!(evaluate_risk(1, &config), RiskLevel::Critical);

    // Boundary: Exactly at critical threshold - 1 -> Critical
    assert_eq!(evaluate_risk(34_559, &config), RiskLevel::Critical);

    // Boundary: Exactly at critical threshold (34,560) -> Warn (since not < 34,560)
    assert_eq!(evaluate_risk(34_560, &config), RiskLevel::Warn);

    // Boundary: Exactly at warning threshold - 1 -> Warn
    assert_eq!(evaluate_risk(120_959, &config), RiskLevel::Warn);

    // Boundary: Exactly at warning threshold (120,960) -> Ok (since not < 120,960)
    assert_eq!(evaluate_risk(120_960, &config), RiskLevel::Ok);

    // Boundary: Above warning threshold -> Ok
    assert_eq!(evaluate_risk(120_961, &config), RiskLevel::Ok);
    assert_eq!(evaluate_risk(i64::MAX, &config), RiskLevel::Ok);
}

// ---------------------------------------------------------------------------
// 6. Instance exists, but references non-existent WASM bytecode hash
// ---------------------------------------------------------------------------
#[tokio::test]
async fn test_edge_case_instance_references_missing_wasm_code_entry() {
    let contract_id = [16u8; 32];
    let contract_strkey = stellar_strkey::Contract(contract_id).to_string();
    let wasm_hash = [88u8; 32];

    let instance_key = derive_contract_instance_key(&contract_id).unwrap();
    let instance_xdr =
        make_instance_entry_xdr(contract_id, ContractExecutable::Wasm(Hash(wasm_hash)));

    // Instance entry is healthy and live
    let instance_raw = RawLedgerEntry::new(&instance_key, &instance_xdr, 6_000_000, 5_390_000);

    // Reader has instance, but NO code entry for wasm_hash
    let mock_reader = MockLedgerReader::new()
        .with_latest_ledger(5_400_000, 22)
        .with_entry(instance_raw);

    let usecase = ScanContractUseCase::new(mock_reader);
    let request = ScanRequest::new(&contract_strkey);

    let report = usecase
        .execute(request)
        .await
        .expect("Scan must not panic when WASM code entry is absent from storage");

    assert_eq!(report.entries.len(), 2);

    // Instance is Ok
    assert_eq!(report.entries[0].key_identifier, "instance");
    assert_eq!(report.entries[0].status, RiskLevel::Ok);

    // Code entry is marked Archived due to missing storage entry
    assert_eq!(report.entries[1].key_identifier, "code_wasm");
    assert_eq!(report.entries[1].status, RiskLevel::Archived);
    assert_eq!(report.entries[1].live_until_ledger, 0);

    // Overall status is Archived
    assert_eq!(report.overall_status, RiskLevel::Archived);
}

// ---------------------------------------------------------------------------
// 7. Malformed Input Validation: Short Strkeys and Invalid XDR Base64
// ---------------------------------------------------------------------------
#[test]
fn test_edge_case_short_strkey_rejected() {
    // 55-character Strkey (valid Strkey is 56 characters)
    let short_strkey = "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2K";
    assert_eq!(short_strkey.len(), 55);

    let result = parse_contract_id(short_strkey);
    assert!(result.is_err());
    match result.unwrap_err() {
        SentinelError::InvalidContractAddress(msg) => {
            assert!(msg.contains("the strkey is invalid") || msg.contains(short_strkey));
        }
        other => panic!("Expected InvalidContractAddress, got: {:?}", other),
    }
}

#[test]
fn test_edge_case_invalid_base64_xdr_key_rejected() {
    let contract_id = [17u8; 32];
    let invalid_xdr_base64 = "NOT_VALID_BASE64_XDR!@#$%^";

    let result = derive_persistent_xdr_key(&contract_id, invalid_xdr_base64);
    assert!(result.is_err());
    match result.unwrap_err() {
        SentinelError::InvalidXdrKey(msg) => {
            assert!(msg.contains(invalid_xdr_base64));
        }
        other => panic!("Expected InvalidXdrKey, got: {:?}", other),
    }
}

// ---------------------------------------------------------------------------
// 8. Contradictory & Low Threshold Edge Cases
// ---------------------------------------------------------------------------
#[tokio::test]
async fn test_edge_case_contradictory_threshold_rejected_in_usecase() {
    let mock_reader = MockLedgerReader::new();
    let usecase = ScanContractUseCase::new(mock_reader);

    let contract_id = [18u8; 32];
    let contract_strkey = stellar_strkey::Contract(contract_id).to_string();

    // Critical threshold > warning threshold (contradictory)
    let contradictory_config = ThresholdConfig::new(50_000, 100_000);
    let request = ScanRequest::new(&contract_strkey).with_threshold_config(contradictory_config);

    let result = usecase.execute(request).await;
    assert!(result.is_err());
    match result.unwrap_err() {
        SentinelError::InvalidThresholdConfig(msg) => {
            assert!(msg.contains("strictly less than"));
        }
        other => panic!("Expected InvalidThresholdConfig, got: {:?}", other),
    }
}

#[test]
fn test_edge_case_scaled_low_threshold_preserves_warn_state() {
    // When warning threshold is low (e.g. 10,000), critical is scaled (e.g. 3,333)
    let low_config = ThresholdConfig::try_new(10_000, 3_333).unwrap();

    // 0 -> Archived
    assert_eq!(evaluate_risk(0, &low_config), RiskLevel::Archived);
    // 2,000 -> Critical (< 3,333)
    assert_eq!(evaluate_risk(2_000, &low_config), RiskLevel::Critical);
    // 3,333 -> Warn (>= 3,333 and < 10,000)
    assert_eq!(evaluate_risk(3_333, &low_config), RiskLevel::Warn);
    // 5,000 -> Warn
    assert_eq!(evaluate_risk(5_000, &low_config), RiskLevel::Warn);
    // 10,000 -> Ok (>= 10,000)
    assert_eq!(evaluate_risk(10_000, &low_config), RiskLevel::Ok);
    // 15,000 -> Ok
    assert_eq!(evaluate_risk(15_000, &low_config), RiskLevel::Ok);
}
