//! Unit and integration tests for Soroban transaction footprint fee calculations.

use std::collections::HashMap;

use sentinel_core::domain::{
    billed_kib_units, scale_bytes_fee, stroops_to_xlm, xdr_encoded_len, FootprintAccess,
    FootprintEntryInput, FootprintEntryKind, FootprintFeeBreakdown, FootprintFeeCalculator,
    FootprintFeeError, BYTES_PER_KIB, STROOPS_PER_XLM, TESTNET_FEE_READ_1KB_STROOPS,
    TESTNET_FEE_WRITE_1KB_STROOPS,
};
use stellar_xdr::curr::{
    BytesM, ConfigSettingContractLedgerCostV0, ContractCodeEntry, ContractCodeEntryExt,
    ContractDataDurability, ContractDataEntry, ContractExecutable, ExtensionPoint, Hash,
    LedgerEntry, LedgerEntryData, LedgerEntryExt, LedgerFootprint, Limits, ScAddress, ScBytes,
    ScContractInstance, ScMap, ScMapEntry, ScSymbol, ScVal, ScVec, VecM, WriteXdr,
};

fn contract_instance_entry(
    contract_id: [u8; 32],
    wasm_hash: [u8; 32],
    storage_entries: Vec<ScMapEntry>,
) -> LedgerEntry {
    let storage = if storage_entries.is_empty() {
        None
    } else {
        Some(ScMap(VecM::try_from(storage_entries).unwrap()))
    };

    LedgerEntry {
        last_modified_ledger_seq: 5_400_000,
        data: LedgerEntryData::ContractData(ContractDataEntry {
            ext: ExtensionPoint::V0,
            contract: ScAddress::Contract(Hash(contract_id)),
            key: ScVal::LedgerKeyContractInstance,
            durability: ContractDataDurability::Persistent,
            val: ScVal::ContractInstance(ScContractInstance {
                executable: ContractExecutable::Wasm(Hash(wasm_hash)),
                storage,
            }),
        }),
        ext: LedgerEntryExt::V0,
    }
}

fn contract_code_entry(wasm_hash: [u8; 32], wasm_len: usize) -> LedgerEntry {
    LedgerEntry {
        last_modified_ledger_seq: 5_380_000,
        data: LedgerEntryData::ContractCode(ContractCodeEntry {
            ext: ContractCodeEntryExt::V0,
            hash: Hash(wasm_hash),
            code: BytesM::try_from(vec![0xABu8; wasm_len]).unwrap(),
        }),
        ext: LedgerEntryExt::V0,
    }
}

fn contract_data_entry(
    contract_id: [u8; 32],
    key: ScVal,
    durability: ContractDataDurability,
    val: ScVal,
) -> LedgerEntry {
    LedgerEntry {
        last_modified_ledger_seq: 5_410_000,
        data: LedgerEntryData::ContractData(ContractDataEntry {
            ext: ExtensionPoint::V0,
            contract: ScAddress::Contract(Hash(contract_id)),
            key,
            durability,
            val,
        }),
        ext: LedgerEntryExt::V0,
    }
}

fn assert_approx_eq(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() < 1e-12,
        "expected {expected}, got {actual}"
    );
}

#[test]
fn ceil_scaling_matches_exact_1kb_boundaries_for_testnet_rates() {
    let calc = FootprintFeeCalculator::testnet_baseline();
    assert_eq!(calc.fee_read_1kb, TESTNET_FEE_READ_1KB_STROOPS);
    assert_eq!(calc.fee_write_1kb, TESTNET_FEE_WRITE_1KB_STROOPS);
    assert_eq!(calc.fee_read_1kb, 1_786);
    assert_eq!(calc.fee_write_1kb, 11_800);

    let cases = [
        (0u64, 0u64),
        (1, 1),
        (512, 1),
        (1_023, 1),
        (1_024, 1),
        (1_025, 2),
        (2_048, 2),
        (2_049, 3),
        (10_240, 10),
        (10_241, 11),
    ];

    for (bytes, expected_kib) in cases {
        assert_eq!(billed_kib_units(bytes), expected_kib);
        assert_eq!(
            calc.calculate_read_only_fee(bytes),
            u128::from(expected_kib) * 1_786
        );
        assert_eq!(
            calc.calculate_read_write_fee(bytes),
            u128::from(expected_kib) * 11_800
        );
    }
}

#[test]
fn verifies_raw_xdr_overhead_of_instance_code_and_arbitrary_storage_keys() {
    let calc = FootprintFeeCalculator::testnet_baseline();
    let contract_id = [0x11u8; 32];
    let wasm_hash = [0x22u8; 32];

    let instance_entry = contract_instance_entry(
        contract_id,
        wasm_hash,
        vec![ScMapEntry {
            key: ScVal::Symbol(ScSymbol("Admin".try_into().unwrap())),
            val: ScVal::Address(ScAddress::Contract(Hash([0x33u8; 32]))),
        }],
    );
    let code_entry = contract_code_entry(wasm_hash, 4_096);
    let counter_key = ScVal::Symbol(ScSymbol("Counter".try_into().unwrap()));
    let counter_entry = contract_data_entry(
        contract_id,
        counter_key.clone(),
        ContractDataDurability::Persistent,
        ScVal::U64(42),
    );
    let composite_key = ScVal::Vec(Some(ScVec(
        VecM::try_from(vec![
            ScVal::Symbol(ScSymbol("Balance".try_into().unwrap())),
            ScVal::Address(ScAddress::Contract(Hash([0x44u8; 32]))),
        ])
        .unwrap(),
    )));
    let balance_entry = contract_data_entry(
        contract_id,
        composite_key.clone(),
        ContractDataDurability::Temporary,
        ScVal::Bytes(ScBytes(BytesM::try_from(vec![0x55u8; 512]).unwrap())),
    );

    let instance_input = FootprintEntryInput::contract_instance(
        contract_id,
        FootprintAccess::ReadOnly,
        Some(instance_entry.clone()),
    );
    let code_input = FootprintEntryInput::contract_code(
        wasm_hash,
        FootprintAccess::ReadOnly,
        Some(code_entry.clone()),
    );
    let counter_input = FootprintEntryInput::storage_key(
        "Counter",
        contract_id,
        counter_key,
        ContractDataDurability::Persistent,
        FootprintAccess::ReadWrite,
        Some(counter_entry.clone()),
    );
    let balance_input = FootprintEntryInput::storage_key(
        "Balance(user)",
        contract_id,
        composite_key,
        ContractDataDurability::Temporary,
        FootprintAccess::ReadWrite,
        Some(balance_entry.clone()),
    );

    let instance_cost = calc.measure_entry(&instance_input).unwrap();
    let code_cost = calc.measure_entry(&code_input).unwrap();
    let counter_cost = calc.measure_entry(&counter_input).unwrap();
    let balance_cost = calc.measure_entry(&balance_input).unwrap();

    // Known exact Stellar XDR key lengths:
    // - LedgerKey::ContractData(LedgerKeyContractInstance): 4 (type) + 36 (ScAddress) + 4 (ScVal) + 4 (durability) = 48 bytes
    // - LedgerKey::ContractCode: 4 (type) + 32 (Hash) = 36 bytes
    // - LedgerKey::ContractData(Symbol("Counter")): 4 + 36 + 4 + 4 + 8 (7 bytes + 1 pad) + 4 = 60 bytes
    assert_eq!(instance_cost.kind, FootprintEntryKind::ContractInstance);
    assert_eq!(instance_cost.key_bytes, 48);
    assert_eq!(
        instance_cost.entry_bytes,
        xdr_encoded_len(&instance_entry).unwrap()
    );
    assert_eq!(
        instance_cost.total_bytes,
        instance_cost.key_bytes + instance_cost.entry_bytes
    );

    assert_eq!(code_cost.kind, FootprintEntryKind::ContractCode);
    assert_eq!(code_cost.key_bytes, 36);
    assert_eq!(code_cost.entry_bytes, xdr_encoded_len(&code_entry).unwrap());
    assert!(code_cost.entry_bytes > 4_096);
    assert_eq!(
        code_cost.total_bytes,
        code_cost.key_bytes + code_cost.entry_bytes
    );

    assert_eq!(counter_cost.kind, FootprintEntryKind::StorageKey);
    assert_eq!(counter_cost.key_bytes, 60);
    assert_eq!(
        counter_cost.entry_bytes,
        xdr_encoded_len(&counter_entry).unwrap()
    );

    assert_eq!(balance_cost.kind, FootprintEntryKind::StorageKey);
    assert_eq!(
        balance_cost.entry_bytes,
        xdr_encoded_len(&balance_entry).unwrap()
    );
    assert!(balance_cost.total_bytes > 512);
}

#[test]
fn verifies_soroban_testnet_resource_fee_baseline_breakdown() {
    // Baseline Soroban testnet rates (CAP-0046):
    // feeRead1KB = 1,786 stroops, feeWrite1KB = 11,800 stroops.
    let calc = FootprintFeeCalculator::new(1_786, 11_800);
    let contract_id = [0xAAu8; 32];
    let wasm_hash = [0xBBu8; 32];

    // Typical contract invocation:
    // - Read-only: ContractCode (10 KiB WASM) + ContractInstance (small metadata)
    // - Read-write: 1 persistent storage entry (2 KiB payload)
    let code_entry = contract_code_entry(wasm_hash, 10_240);
    let instance_entry = contract_instance_entry(contract_id, wasm_hash, vec![]);
    let state_key = ScVal::Symbol(ScSymbol("LedgerState".try_into().unwrap()));
    let state_entry = contract_data_entry(
        contract_id,
        state_key.clone(),
        ContractDataDurability::Persistent,
        ScVal::Bytes(ScBytes(BytesM::try_from(vec![0x7Fu8; 2_048]).unwrap())),
    );

    let inputs = vec![
        FootprintEntryInput::contract_code(wasm_hash, FootprintAccess::ReadOnly, Some(code_entry)),
        FootprintEntryInput::contract_instance(
            contract_id,
            FootprintAccess::ReadOnly,
            Some(instance_entry),
        ),
        FootprintEntryInput::storage_key(
            "LedgerState",
            contract_id,
            state_key,
            ContractDataDurability::Persistent,
            FootprintAccess::ReadWrite,
            Some(state_entry),
        ),
    ];

    let breakdown = calc.calculate(&inputs).unwrap();

    // Read-only partition:
    // code_key (36) + code_entry (10_240 + 52 = 10_292) = 10_328 bytes
    // instance_key (48) + instance_entry (100) = 148 bytes
    // Sum = 10_476 bytes -> ceil(10_476 / 1024) = 11 KiB -> 11 * 1_786 = 19_646 stroops
    assert_eq!(breakdown.read_only.access, FootprintAccess::ReadOnly);
    assert_eq!(breakdown.read_only.entry_count, 2);
    assert_eq!(breakdown.read_only.rate_per_1kb_stroops, 1_786);
    let expected_ro_bytes = breakdown.entries[0].total_bytes + breakdown.entries[1].total_bytes;
    let expected_ro_kib = expected_ro_bytes.div_ceil(BYTES_PER_KIB);
    let expected_ro_stroops = u128::from(expected_ro_kib) * 1_786;
    assert_eq!(breakdown.read_only.total_bytes, expected_ro_bytes);
    assert_eq!(breakdown.read_only.billed_kib_units, expected_ro_kib);
    assert_eq!(breakdown.read_only.billed_kib_units, 11);
    assert_eq!(breakdown.read_only.fee_stroops, expected_ro_stroops);
    assert_eq!(breakdown.read_only.fee_stroops, 19_646);
    assert_approx_eq(breakdown.read_only.fee_xlm, 0.0019646);

    // Read-write partition:
    // state_key (64) + state_entry (2_048 + XDR headers) -> 3 KiB -> 3 * 11_800 = 35_400 stroops
    assert_eq!(breakdown.read_write.access, FootprintAccess::ReadWrite);
    assert_eq!(breakdown.read_write.entry_count, 1);
    assert_eq!(breakdown.read_write.rate_per_1kb_stroops, 11_800);
    let expected_rw_bytes = breakdown.entries[2].total_bytes;
    let expected_rw_kib = expected_rw_bytes.div_ceil(BYTES_PER_KIB);
    let expected_rw_stroops = u128::from(expected_rw_kib) * 11_800;
    assert_eq!(breakdown.read_write.total_bytes, expected_rw_bytes);
    assert_eq!(breakdown.read_write.billed_kib_units, expected_rw_kib);
    assert_eq!(breakdown.read_write.billed_kib_units, 3);
    assert_eq!(breakdown.read_write.fee_stroops, expected_rw_stroops);
    assert_eq!(breakdown.read_write.fee_stroops, 35_400);
    assert_approx_eq(breakdown.read_write.fee_xlm, 0.00354);

    // Combined totals:
    assert_eq!(
        breakdown.total_bytes,
        breakdown.read_only.total_bytes + breakdown.read_write.total_bytes
    );
    assert_eq!(breakdown.total_fee_stroops, 19_646 + 35_400);
    assert_eq!(breakdown.total_fee_stroops, 55_046);
    assert_approx_eq(breakdown.total_fee_xlm, 0.0055046);

    // Verify partition and kind iterators.
    assert_eq!(breakdown.read_only_entries().count(), 2);
    assert_eq!(breakdown.read_write_entries().count(), 1);
    assert_eq!(
        breakdown
            .entries_by_kind(FootprintEntryKind::ContractCode)
            .count(),
        1
    );
    assert_eq!(
        breakdown
            .entries_by_kind(FootprintEntryKind::ContractInstance)
            .count(),
        1
    );
    assert_eq!(
        breakdown
            .entries_by_kind(FootprintEntryKind::StorageKey)
            .count(),
        1
    );
}

#[test]
fn verifies_protocol_22_adjusted_testnet_rates_via_ledger_cost_config() {
    // Protocol 22 / low-bucket-list testnet configuration:
    // feeRead1KB = 1,624 stroops, effective write fee = 10,000 stroops / KiB.
    let config = ConfigSettingContractLedgerCostV0 {
        ledger_max_read_ledger_entries: 200,
        ledger_max_read_bytes: 200_000,
        ledger_max_write_ledger_entries: 100,
        ledger_max_write_bytes: 132_096,
        tx_max_read_ledger_entries: 40,
        tx_max_read_bytes: 200_000,
        tx_max_write_ledger_entries: 25,
        tx_max_write_bytes: 66_560,
        fee_read_ledger_entry: 6_250,
        fee_write_ledger_entry: 10_000,
        fee_read1_kb: 1_624,
        bucket_list_target_size_bytes: 30_000_000_000,
        write_fee1_kb_bucket_list_low: 1_000,
        write_fee1_kb_bucket_list_high: 10_000,
        bucket_list_write_fee_growth_factor: 50,
    };

    let calc = FootprintFeeCalculator::from_ledger_cost_config(&config, 10_000).unwrap();
    assert_eq!(calc.fee_read_1kb, 1_624);
    assert_eq!(calc.fee_write_1kb, 10_000);

    // 4,096 read bytes (4 KiB) + 2,049 write bytes (3 KiB)
    let ro_fee = calc.calculate_read_only_fee(4_096);
    let rw_fee = calc.calculate_read_write_fee(2_049);
    assert_eq!(ro_fee, 4 * 1_624);
    assert_eq!(rw_fee, 3 * 10_000);
    assert_approx_eq(stroops_to_xlm(ro_fee + rw_fee), 36_496.0 / 10_000_000.0);

    // Negative feeRead1KB in malformed config must be rejected.
    let mut bad_config = config;
    bad_config.fee_read1_kb = -1;
    assert!(matches!(
        FootprintFeeCalculator::from_ledger_cost_config(&bad_config, 10_000),
        Err(FootprintFeeError::InvalidFeeRate(_))
    ));
}

#[test]
fn aggregates_sub_1kb_entries_before_applying_ceil_scaling() {
    let calc = FootprintFeeCalculator::testnet_baseline();
    let contract_id = [0x55u8; 32];

    // Create 3 small read-only keys (each < 250 bytes total) whose combined sum is < 1024 bytes.
    let small_entries: Vec<FootprintEntryInput> = (0u32..3)
        .map(|idx| {
            let key = ScVal::U32(idx);
            let entry = contract_data_entry(
                contract_id,
                key.clone(),
                ContractDataDurability::Persistent,
                ScVal::U64(u64::from(idx)),
            );
            FootprintEntryInput::storage_key(
                format!("k{idx}"),
                contract_id,
                key,
                ContractDataDurability::Persistent,
                FootprintAccess::ReadOnly,
                Some(entry),
            )
        })
        .collect();

    let breakdown = calc.calculate(&small_entries).unwrap();
    assert!(breakdown.read_only.total_bytes < BYTES_PER_KIB);
    assert_eq!(breakdown.read_only.billed_kib_units, 1);
    assert_eq!(breakdown.read_only.fee_stroops, 1_786);

    // Individual standalone fees each round up to 1 KiB (3 * 1_786), while aggregate shares 1 KiB.
    let standalone_sum: u128 = breakdown
        .entries
        .iter()
        .map(|entry| entry.standalone_fee_stroops)
        .sum();
    assert_eq!(standalone_sum, 3 * 1_786);
    assert!(breakdown.read_only.fee_stroops < standalone_sum);

    // Now add entries whose combined sum exceeds 1024 bytes -> bills 2 KiB units.
    let mut crossing_entries = small_entries;
    let payload_key = ScVal::U32(99);
    let payload_entry = contract_data_entry(
        contract_id,
        payload_key.clone(),
        ContractDataDurability::Persistent,
        ScVal::Bytes(ScBytes(BytesM::try_from(vec![1u8; 700]).unwrap())),
    );
    crossing_entries.push(FootprintEntryInput::storage_key(
        "k_large",
        contract_id,
        payload_key,
        ContractDataDurability::Persistent,
        FootprintAccess::ReadOnly,
        Some(payload_entry),
    ));

    let crossed = calc.calculate(&crossing_entries).unwrap();
    assert!(crossed.read_only.total_bytes > BYTES_PER_KIB);
    assert!(crossed.read_only.total_bytes <= 2 * BYTES_PER_KIB);
    assert_eq!(crossed.read_only.billed_kib_units, 2);
    assert_eq!(crossed.read_only.fee_stroops, 2 * 1_786);
}

#[test]
fn calculates_directly_from_xdr_ledger_footprint_and_entry_map() {
    let calc = FootprintFeeCalculator::testnet_baseline();
    let contract_id = [0x77u8; 32];
    let wasm_hash = [0x88u8; 32];

    let instance_input = FootprintEntryInput::contract_instance(
        contract_id,
        FootprintAccess::ReadOnly,
        Some(contract_instance_entry(contract_id, wasm_hash, vec![])),
    );
    let code_input = FootprintEntryInput::contract_code(wasm_hash, FootprintAccess::ReadOnly, None);
    let write_key_val = ScVal::Symbol(ScSymbol("TotalSupply".try_into().unwrap()));
    let write_input = FootprintEntryInput::storage_key(
        "TotalSupply",
        contract_id,
        write_key_val.clone(),
        ContractDataDurability::Persistent,
        FootprintAccess::ReadWrite,
        Some(contract_data_entry(
            contract_id,
            write_key_val,
            ContractDataDurability::Persistent,
            ScVal::I128(stellar_xdr::curr::Int128Parts {
                hi: 0,
                lo: 1_000_000,
            }),
        )),
    );

    let footprint = LedgerFootprint {
        read_only: VecM::try_from(vec![instance_input.key.clone(), code_input.key.clone()])
            .unwrap(),
        read_write: VecM::try_from(vec![write_input.key.clone()]).unwrap(),
    };

    let mut entries_by_key = HashMap::new();
    entries_by_key.insert(
        instance_input.key.clone(),
        instance_input.entry.clone().unwrap(),
    );
    // Intentionally omit `code_input.key` from map to verify key-only fallback (`entry_bytes = 0`).
    entries_by_key.insert(write_input.key.clone(), write_input.entry.clone().unwrap());

    let breakdown = calc
        .calculate_ledger_footprint(&footprint, &entries_by_key)
        .unwrap();

    assert_eq!(breakdown.read_only.entry_count, 2);
    assert_eq!(breakdown.read_write.entry_count, 1);
    assert_eq!(
        breakdown.entries[0].kind,
        FootprintEntryKind::ContractInstance
    );
    assert_eq!(breakdown.entries[1].kind, FootprintEntryKind::ContractCode);
    assert_eq!(breakdown.entries[1].key_bytes, 36);
    assert_eq!(breakdown.entries[1].entry_bytes, 0);
    assert_eq!(breakdown.entries[2].kind, FootprintEntryKind::StorageKey);
    assert_eq!(
        breakdown.total_fee_stroops,
        breakdown.read_only.fee_stroops + breakdown.read_write.fee_stroops
    );
}

#[test]
fn parses_base64_xdr_inputs_and_rejects_malformed_xdr() {
    let calc = FootprintFeeCalculator::testnet_baseline();
    let contract_id = [0x99u8; 32];
    let wasm_hash = [0x66u8; 32];

    let raw_input = FootprintEntryInput::contract_instance(
        contract_id,
        FootprintAccess::ReadOnly,
        Some(contract_instance_entry(contract_id, wasm_hash, vec![])),
    );
    let key_b64 = raw_input.key.to_xdr_base64(Limits::none()).unwrap();
    let entry_b64 = raw_input
        .entry
        .as_ref()
        .unwrap()
        .to_xdr_base64(Limits::none())
        .unwrap();

    let decoded = FootprintEntryInput::from_xdr_base64(
        "instance_from_rpc",
        FootprintAccess::ReadOnly,
        &key_b64,
        Some(&entry_b64),
    )
    .unwrap();
    assert_eq!(decoded.kind, FootprintEntryKind::ContractInstance);

    let from_raw = calc.calculate(&[raw_input]).unwrap();
    let from_b64 = calc.calculate(&[decoded]).unwrap();
    assert_eq!(
        from_raw.read_only.total_bytes,
        from_b64.read_only.total_bytes
    );
    assert_eq!(from_raw.total_fee_stroops, from_b64.total_fee_stroops);

    assert!(matches!(
        FootprintEntryInput::from_xdr_base64("bad_key", FootprintAccess::ReadOnly, "!!!", None),
        Err(FootprintFeeError::XdrDecode(_))
    ));
    assert!(matches!(
        FootprintEntryInput::from_xdr_base64(
            "bad_entry",
            FootprintAccess::ReadWrite,
            &key_b64,
            Some("not-valid-xdr")
        ),
        Err(FootprintFeeError::XdrDecode(_))
    ));
}

#[test]
fn handles_empty_footprint_zero_rates_large_values_and_json_roundtrip() {
    // Empty footprint -> zero cost across both partitions.
    let calc = FootprintFeeCalculator::testnet_baseline();
    let empty = calc.calculate(&[]).unwrap();
    assert_eq!(empty.read_only.entry_count, 0);
    assert_eq!(empty.read_only.total_bytes, 0);
    assert_eq!(empty.read_only.billed_kib_units, 0);
    assert_eq!(empty.read_only.fee_stroops, 0);
    assert_eq!(empty.read_only.fee_xlm, 0.0);
    assert_eq!(empty.read_write.entry_count, 0);
    assert_eq!(empty.read_write.total_bytes, 0);
    assert_eq!(empty.read_write.billed_kib_units, 0);
    assert_eq!(empty.read_write.fee_stroops, 0);
    assert_eq!(empty.read_write.fee_xlm, 0.0);
    assert_eq!(empty.total_bytes, 0);
    assert_eq!(empty.total_fee_stroops, 0);
    assert_eq!(empty.total_fee_xlm, 0.0);
    assert!(empty.entries.is_empty());

    // Zero fee rates preserve byte accounting while costing 0 stroops.
    let free_calc = FootprintFeeCalculator::new(0, 0);
    let key_only = FootprintEntryInput::contract_code([1u8; 32], FootprintAccess::ReadWrite, None);
    let free_breakdown = free_calc.calculate(&[key_only]).unwrap();
    assert_eq!(free_breakdown.read_write.total_bytes, 36);
    assert_eq!(free_breakdown.read_write.billed_kib_units, 1);
    assert_eq!(free_breakdown.read_write.fee_stroops, 0);
    assert_eq!(free_breakdown.total_fee_xlm, 0.0);

    // Wide u128 scaling preserves exact precision for very large rates and byte counts.
    let large_fee = scale_bytes_fee(u64::MAX, u64::MAX);
    assert_eq!(
        large_fee,
        u128::from(u64::MAX.div_ceil(BYTES_PER_KIB)) * u128::from(u64::MAX)
    );
    assert!(large_fee > u128::from(u64::MAX));
    assert_eq!(STROOPS_PER_XLM, 10_000_000);

    // Serde JSON roundtrip for FootprintFeeBreakdown.
    let json = serde_json::to_string(&free_breakdown).unwrap();
    let roundtripped: FootprintFeeBreakdown = serde_json::from_str(&json).unwrap();
    assert_eq!(roundtripped, free_breakdown);
}
