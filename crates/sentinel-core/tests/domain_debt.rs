mod domain {
    mod debt {
        use std::num::{NonZeroU32, NonZeroU64};

        use sentinel_core::adapters::{derive_contract_instance_key, derive_persistent_symbol_key};
        use sentinel_core::domain::{Durability, EntryType, RentDebtCalculator};
        use sentinel_core::{MockLedgerReader, RawLedgerEntry, ScanContractUseCase, ScanRequest};
        use stellar_xdr::curr::{
            ContractDataDurability, ContractDataEntry, ContractExecutable, ExtensionPoint, Hash,
            LedgerEntryData, Limits, ScAddress, ScContractInstance, ScSymbol, ScVal, WriteXdr,
        };

        fn calculator() -> RentDebtCalculator {
            RentDebtCalculator {
                write_fee_per_kib_stroops: 101,
                write_fee_per_entry_stroops: 10,
                persistent_rent_rate_denominator: NonZeroU64::new(5).unwrap(),
                min_persistent_ttl: NonZeroU32::new(20).unwrap(),
            }
        }

        #[test]
        fn rounds_each_fee_up_and_only_modeled_arrears_grow_with_expiry() {
            for (expired, arrears) in [(1, 11), (10, 101), (100, 1010)] {
                let debt = calculator()
                    .calculate(Durability::Instance, 512, 1000, 1000 + expired)
                    .unwrap();
                assert_eq!(debt.expired_ledgers, expired);
                assert_eq!(debt.arrears_recovery_fee_stroops, arrears);
                assert_eq!(debt.new_lifetime_extension_fee_stroops, 202);
                assert_eq!(debt.ttl_write_fee_stroops, 17);
                assert_eq!(debt.restoration_rent_stroops, 219);
                assert_eq!(debt.modeled_total_stroops, 219 + arrears);
            }
        }

        #[test]
        fn skips_missing_live_and_temporary_entries() {
            let calc = calculator();
            for (durability, size, ttl) in [
                (Durability::Persistent, 512, 101),
                (Durability::Persistent, 512, 100),
                (Durability::Persistent, 512, 0),
                (Durability::Persistent, 0, 99),
                (Durability::Temporary, 512, 99),
            ] {
                assert!(calc.calculate(durability, size, ttl, 100).is_none());
            }
        }

        #[test]
        fn retains_precision_with_large_inputs() {
            let calc = RentDebtCalculator {
                write_fee_per_kib_stroops: u64::MAX,
                ..calculator()
            };
            let debt = calc
                .calculate(Durability::Persistent, u32::MAX, 1, u32::MAX)
                .unwrap();
            assert!(debt.arrears_recovery_fee_stroops > u128::from(u64::MAX));
            assert_eq!(
                debt.modeled_total_stroops,
                debt.arrears_recovery_fee_stroops + debt.restoration_rent_stroops
            );
        }

        #[tokio::test]
        async fn scan_quotes_instance_and_persistent_data_with_known_ttls() {
            let contract = [6; 32];
            let instance_key = derive_contract_instance_key(&contract).unwrap();
            let data_key = derive_persistent_symbol_key(&contract, "DATA").unwrap();
            let xdr = |key, val| {
                LedgerEntryData::ContractData(ContractDataEntry {
                    ext: ExtensionPoint::V0,
                    contract: ScAddress::Contract(Hash(contract)),
                    key,
                    durability: ContractDataDurability::Persistent,
                    val,
                })
                .to_xdr_base64(Limits::none())
                .unwrap()
            };
            let instance = xdr(
                ScVal::LedgerKeyContractInstance,
                ScVal::ContractInstance(ScContractInstance {
                    executable: ContractExecutable::StellarAsset,
                    storage: None,
                }),
            );
            let data = xdr(
                ScVal::Symbol(ScSymbol("DATA".try_into().unwrap())),
                ScVal::U32(4),
            );
            let reader = MockLedgerReader::new()
                .with_latest_ledger(1000, 22)
                .with_entries(vec![
                    RawLedgerEntry::new(instance_key, instance, 999, 888),
                    RawLedgerEntry::new(data_key, data, 900, 888),
                ]);
            let report = ScanContractUseCase::new(reader)
                .execute(
                    ScanRequest::new(stellar_strkey::Contract(contract).to_string())
                        .with_persistent_symbols(vec!["DATA".to_owned(), "MISSING".to_owned()]),
                )
                .await
                .unwrap();

            let calc = calculator();
            let debts: Vec<_> = report.rent_debts(&calc).collect();
            assert_eq!(debts.len(), 2);
            assert_eq!(debts[0].0.entry_type, EntryType::ContractInstance);
            assert_eq!(debts[0].1.expired_ledgers, 1);
            assert_eq!(debts[1].0.entry_type, EntryType::PersistentData);
            assert_eq!(debts[1].1.expired_ledgers, 100);
            assert!(
                debts[1].1.arrears_recovery_fee_stroops > debts[0].1.arrears_recovery_fee_stroops
            );
        }
    }
}
