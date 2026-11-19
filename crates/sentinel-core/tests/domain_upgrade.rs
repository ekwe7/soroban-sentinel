use sentinel_core::adapters::{derive_contract_code_key, derive_contract_instance_key};
use sentinel_core::domain::DEFAULT_MINIMUM_TTL_LEDGERS;
use sentinel_core::ports::mock::MockLedgerReader;
use sentinel_core::ports::reader::RawLedgerEntry;
use sentinel_core::usecases::{ScanContractUseCase, ScanRequest};
use stellar_xdr::curr::{
    ContractCodeEntry, ContractCodeEntryExt, ContractDataDurability, ContractDataEntry,
    ContractExecutable, ExtensionPoint, Hash, LedgerEntryData, Limits, ScAddress,
    ScContractInstance, ScVal, WriteXdr,
};

const CONTRACT_ID: [u8; 32] = [9; 32];
const LATEST_LEDGER: u32 = 5_400_000;

fn make_instance_entry(hash: [u8; 32]) -> String {
    LedgerEntryData::ContractData(ContractDataEntry {
        ext: ExtensionPoint::V0,
        contract: ScAddress::Contract(Hash(CONTRACT_ID)),
        key: ScVal::LedgerKeyContractInstance,
        durability: ContractDataDurability::Persistent,
        val: ScVal::ContractInstance(ScContractInstance {
            executable: ContractExecutable::Wasm(Hash(hash)),
            storage: None,
        }),
    })
    .to_xdr_base64(Limits::none())
    .unwrap()
}

fn make_code_entry(hash: [u8; 32], code: &[u8]) -> String {
    LedgerEntryData::ContractCode(ContractCodeEntry {
        ext: ContractCodeEntryExt::V0,
        hash: Hash(hash),
        code: code.to_vec().try_into().unwrap(),
    })
    .to_xdr_base64(Limits::none())
    .unwrap()
}

async fn scan_code(hash: [u8; 32], code: &[u8], request: ScanRequest) -> sentinel_core::TTLReport {
    let instance_key = derive_contract_instance_key(&CONTRACT_ID).unwrap();
    let code_key = derive_contract_code_key(&hash).unwrap();
    let reader = MockLedgerReader::new()
        .with_latest_ledger(LATEST_LEDGER, 22)
        .with_entries(vec![
            RawLedgerEntry::new(
                instance_key,
                make_instance_entry(hash),
                6_000_000,
                LATEST_LEDGER,
            ),
            RawLedgerEntry::new(
                code_key,
                make_code_entry(hash, code),
                6_000_000,
                LATEST_LEDGER,
            ),
        ]);
    ScanContractUseCase::new(reader)
        .execute(request)
        .await
        .unwrap()
}

#[tokio::test]
async fn scan_emits_upgrade_with_bytecode_size_and_minimum_ttl_rent_deltas() {
    let previous = scan_code(
        [1; 32],
        &[1; 128],
        ScanRequest::new(stellar_strkey::Contract(CONTRACT_ID).to_string()),
    )
    .await
    .contract_code_state
    .expect("baseline code state should be included");

    let report = scan_code(
        [2; 32],
        &[2; 256],
        ScanRequest::new(stellar_strkey::Contract(CONTRACT_ID).to_string())
            .with_baseline_code_state(previous)
            .with_minimum_ttl_ledgers(500),
    )
    .await;

    let event = report
        .upgrade_event
        .expect("changed WASM hash should emit an upgrade event");
    assert_eq!(event.previous_code_hash, [1; 32]);
    assert_eq!(event.new_code_hash, [2; 32]);
    assert_eq!(event.ledger_height, LATEST_LEDGER);
    assert_eq!(event.bytecode_size_delta, 128);
    assert_eq!(event.minimum_ttl_rent_delta, 128 * 500);
    assert_eq!(report.contract_code_state.unwrap().bytecode_size_bytes, 256);
}

#[tokio::test]
async fn scan_does_not_emit_upgrade_without_a_baseline_or_hash_change() {
    let contract_id = stellar_strkey::Contract(CONTRACT_ID).to_string();
    let baseline = scan_code([3; 32], &[1; 64], ScanRequest::new(&contract_id))
        .await
        .contract_code_state
        .unwrap();

    let unchanged = scan_code(
        [3; 32],
        &[1; 64],
        ScanRequest::new(&contract_id).with_baseline_code_state(baseline),
    )
    .await;
    assert!(unchanged.upgrade_event.is_none());

    let no_baseline = scan_code([4; 32], &[2; 80], ScanRequest::new(&contract_id)).await;
    assert!(no_baseline.upgrade_event.is_none());
}

#[test]
fn default_minimum_ttl_is_documented_and_configurable() {
    assert_eq!(DEFAULT_MINIMUM_TTL_LEDGERS, 4_096);
    assert_eq!(
        ScanRequest::new("contract").minimum_ttl_ledgers,
        DEFAULT_MINIMUM_TTL_LEDGERS
    );
    let configured = ScanRequest::new("contract").with_minimum_ttl_ledgers(10);
    assert_eq!(configured.minimum_ttl_ledgers, 10);
}
