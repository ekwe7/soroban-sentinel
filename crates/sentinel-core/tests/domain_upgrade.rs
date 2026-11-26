use std::num::{NonZeroU32, NonZeroU64};

use sentinel_core::adapters::{derive_contract_code_key, derive_contract_instance_key};
use sentinel_core::domain::{ContractUpgradeBaseline, RentDebtCalculator};
use sentinel_core::ports::mock::MockLedgerReader;
use sentinel_core::ports::reader::RawLedgerEntry;
use sentinel_core::usecases::{ScanContractUseCase, ScanRequest};
use stellar_xdr::curr::{
    ContractCodeEntry, ContractCodeEntryExt, ContractDataDurability, ContractDataEntry,
    ContractExecutable, ExtensionPoint, Hash, LedgerEntry, LedgerEntryData, LedgerEntryExt, Limits,
    ScAddress, ScContractInstance, ScVal, WriteXdr,
};

fn make_instance_xdr(contract_id: [u8; 32], code_hash: [u8; 32]) -> String {
    let data = LedgerEntryData::ContractData(ContractDataEntry {
        ext: ExtensionPoint::V0,
        contract: ScAddress::Contract(Hash(contract_id)),
        key: ScVal::LedgerKeyContractInstance,
        durability: ContractDataDurability::Persistent,
        val: ScVal::ContractInstance(ScContractInstance {
            executable: ContractExecutable::Wasm(Hash(code_hash)),
            storage: None,
        }),
    });
    data.to_xdr_base64(Limits::none()).unwrap()
}

fn make_code_xdr(code_hash: [u8; 32], bytecode: &[u8]) -> String {
    let entry = LedgerEntry {
        last_modified_ledger_seq: 5_390_000,
        data: LedgerEntryData::ContractCode(ContractCodeEntry {
            ext: ContractCodeEntryExt::V0,
            hash: Hash(code_hash),
            code: bytecode.to_vec().try_into().unwrap(),
        }),
        ext: LedgerEntryExt::V0,
    };
    entry.to_xdr_base64(Limits::none()).unwrap()
}

#[tokio::test]
async fn scan_emits_upgrade_event_with_bytecode_and_minimum_ttl_rent_deltas() {
    let contract_hash = [7; 32];
    let baseline_code_hash = [8; 32];
    let current_code_hash = [9; 32];
    let contract_id = stellar_strkey::Contract(contract_hash).to_string();
    let instance_key = derive_contract_instance_key(&contract_hash).unwrap();
    let code_key = derive_contract_code_key(&current_code_hash).unwrap();
    let reader = MockLedgerReader::new()
        .with_latest_ledger(5_400_000, 22)
        .with_entry(RawLedgerEntry::new(
            &instance_key,
            make_instance_xdr(contract_hash, current_code_hash),
            5_500_000,
            5_390_000,
        ))
        .with_entry(RawLedgerEntry::new(
            &code_key,
            make_code_xdr(current_code_hash, &[0xDE, 0xAD, 0xBE, 0xEF]),
            5_500_000,
            5_390_000,
        ));
    let rent_calculator = RentDebtCalculator {
        write_fee_per_kib_stroops: 1_024,
        write_fee_per_entry_stroops: 0,
        persistent_rent_rate_denominator: NonZeroU64::new(1).unwrap(),
        min_persistent_ttl: NonZeroU32::new(100).unwrap(),
    };

    let report = ScanContractUseCase::new(reader)
        .with_upgrade_baseline(
            ContractUpgradeBaseline::new(baseline_code_hash, 2),
            rent_calculator,
        )
        .execute(ScanRequest::new(&contract_id))
        .await
        .expect("scan should detect the code change");

    assert_eq!(report.contract_code_hash, Some("09".repeat(32)));
    assert_eq!(report.contract_bytecode_size, Some(4));
    let event = report
        .contract_upgrade
        .expect("upgrade event should be present");
    assert_eq!(event.previous_code_hash, "08".repeat(32));
    assert_eq!(event.new_code_hash, "09".repeat(32));
    assert_eq!(event.ledger_height, 5_400_000);
    assert_eq!(event.bytecode_size_delta, 2);
    assert_eq!(event.minimum_ttl_rent_delta_stroops, 200);
}

#[tokio::test]
async fn upgrade_tracking_reports_a_missing_contract_code_entry() {
    let contract_hash = [7; 32];
    let baseline_code_hash = [8; 32];
    let current_code_hash = [9; 32];
    let contract_id = stellar_strkey::Contract(contract_hash).to_string();
    let instance_key = derive_contract_instance_key(&contract_hash).unwrap();
    let reader = MockLedgerReader::new()
        .with_latest_ledger(5_400_000, 22)
        .with_entry(RawLedgerEntry::new(
            &instance_key,
            make_instance_xdr(contract_hash, current_code_hash),
            5_500_000,
            5_390_000,
        ));
    let rent_calculator = RentDebtCalculator {
        write_fee_per_kib_stroops: 1_024,
        write_fee_per_entry_stroops: 0,
        persistent_rent_rate_denominator: NonZeroU64::new(1).unwrap(),
        min_persistent_ttl: NonZeroU32::new(100).unwrap(),
    };

    let error = ScanContractUseCase::new(reader)
        .with_upgrade_baseline(
            ContractUpgradeBaseline::new(baseline_code_hash, 2),
            rent_calculator,
        )
        .execute(ScanRequest::new(&contract_id))
        .await
        .expect_err("missing code must not silently suppress upgrade tracking");

    assert!(error
        .to_string()
        .contains("contract code entry is missing during upgrade tracking"));
}
