mod domain {
    mod cold_storage {
        use std::collections::HashMap;

        use sentinel_core::adapters::derive_persistent_symbol_key;
        use sentinel_core::domain::cold_storage::{
            recommend_archival, ArchivalPolicy, ExtensionHistory,
        };
        use sentinel_core::domain::{Durability, EntryType, StorageEntry};
        use sentinel_core::usecases::scan::{ScanContractUseCase, ScanRequest};
        use sentinel_core::{MockLedgerReader, RawLedgerEntry};
        use stellar_xdr::curr::{
            ContractDataDurability, ContractDataEntry, ExtensionPoint, Hash, LedgerEntryData,
            Limits, ScAddress, ScBytes, ScSymbol, ScVal, WriteXdr,
        };

        const LATEST: u32 = 5_000_000;
        const TTL: u32 = LATEST + 100;
        const YEAR_SECONDS: f64 = 365.0 * 86_400.0;

        fn policy() -> ArchivalPolicy {
            ArchivalPolicy {
                minimum_byte_size: 1024,
                minimum_inactive_ledgers: 1000,
                projection_years: 2,
                rent_stroops_per_kib_ledger: 10.0,
                restoration_base_stroops: 100.0,
                restoration_stroops_per_kib: 50.0,
            }
        }

        fn history() -> ExtensionHistory {
            ExtensionHistory {
                first_ledger: LATEST - 1000,
                last_ledger: LATEST,
                extension_count: 0,
            }
        }

        #[test]
        fn projects_net_savings_after_prepaid_ttl_and_one_restore() {
            let entry = StorageEntry::new(
                "legacy",
                EntryType::PersistentData,
                Durability::Persistent,
                2048,
                TTL,
                LATEST - 1000,
            );
            let result = recommend_archival(&entry, history(), &policy(), LATEST, 5.0).unwrap();
            let renewals = (2.0 * YEAR_SECONDS / 5.0) as u64 - 100;
            assert_eq!(result.projected_renewal_ledgers, renewals);
            assert_eq!(result.rent_stroops_per_ledger, 20.0);
            assert_eq!(result.projected_rent_stroops, renewals as f64 * 20.0);
            assert_eq!(result.restoration_cost_stroops, 200.0);
            assert_eq!(
                result.projected_savings_stroops,
                result.projected_rent_stroops - 200.0
            );

            for restore in [
                result.projected_rent_stroops - 100.0,
                result.projected_rent_stroops,
            ] {
                let expensive = ArchivalPolicy {
                    restoration_base_stroops: restore,
                    ..policy()
                };
                assert!(recommend_archival(&entry, history(), &expensive, LATEST, 5.0).is_none());
            }
        }

        #[test]
        fn rejects_protected_recent_small_and_expired_entries() {
            use Durability::*;
            use EntryType::*;

            for (kind, durability, bytes, ttl, modified) in [
                (ContractInstance, Persistent, 2048, TTL, LATEST - 1000),
                (ContractCode, Persistent, 2048, TTL, LATEST - 1000),
                (PersistentData, Temporary, 2048, TTL, LATEST - 1000),
                (PersistentData, Persistent, 1023, TTL, LATEST - 1000),
                (PersistentData, Persistent, 2048, LATEST - 1, LATEST - 1000),
                (PersistentData, Persistent, 2048, TTL, LATEST - 999),
                (PersistentData, Persistent, 2048, TTL, 0),
            ] {
                let entry = StorageEntry::new("key", kind, durability, bytes, ttl, modified);
                assert!(recommend_archival(&entry, history(), &policy(), LATEST, 5.0).is_none());
            }
        }

        #[test]
        fn rejects_active_short_and_stale_histories() {
            let entry = StorageEntry::new(
                "legacy",
                EntryType::PersistentData,
                Durability::Persistent,
                2048,
                TTL,
                LATEST - 1000,
            );
            for observation in [
                ExtensionHistory {
                    extension_count: 1,
                    ..history()
                },
                ExtensionHistory {
                    first_ledger: LATEST - 999,
                    ..history()
                },
                ExtensionHistory {
                    last_ledger: LATEST - 1,
                    ..history()
                },
            ] {
                assert!(recommend_archival(&entry, observation, &policy(), LATEST, 5.0).is_none());
            }
        }

        #[tokio::test]
        async fn scan_recommends_only_with_history_and_deduplicates_matching_keys() {
            let contract = [7; 32];
            let key = derive_persistent_symbol_key(&contract, "LEGACY").unwrap();
            let xdr = LedgerEntryData::ContractData(ContractDataEntry {
                ext: ExtensionPoint::V0,
                contract: ScAddress::Contract(Hash(contract)),
                key: ScVal::Symbol(ScSymbol("LEGACY".try_into().unwrap())),
                durability: ContractDataDurability::Persistent,
                val: ScVal::Bytes(ScBytes(vec![0; 2048].try_into().unwrap())),
            })
            .to_xdr_base64(Limits::none())
            .unwrap();
            let reader = MockLedgerReader::new()
                .with_latest_ledger(LATEST, 22)
                .with_entry(RawLedgerEntry::new(&key, &xdr, TTL, LATEST - 1000));
            let request = ScanRequest::new(stellar_strkey::Contract(contract).to_string())
                .with_persistent_symbols(vec!["LEGACY".to_owned(), "LEGACY".to_owned()]);
            let scan = ScanContractUseCase::new(reader);
            let report = scan.execute(request.clone()).await.unwrap();
            assert!(report.archival_recommendations.is_empty());

            let report = scan
                .with_archival_analysis(policy(), HashMap::from([(key, history())]))
                .execute(request)
                .await
                .unwrap();
            assert_eq!(report.archival_recommendations.len(), 1);
            assert_eq!(
                report.archival_recommendations[0].key_identifier,
                "persistent:LEGACY"
            );
        }
    }
}
