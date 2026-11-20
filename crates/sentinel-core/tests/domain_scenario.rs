mod domain {
    mod scenario {
        use sentinel_core::domain::{
            evaluate_protocol_scenario, NetworkRentParameters, RiskLevel, ScenarioEntry,
            ScenarioError, ThresholdConfig,
        };
        use sentinel_core::usecases::{ProtocolScenarioUseCase, StorageEntryReport, TTLReport};
        use sentinel_core::{Durability, EntryType};

        fn parameters(
            rate: f64,
            multiplier: f64,
            persistent_ttl: u32,
            warning: u32,
            critical: u32,
            ledgers_per_month: u32,
        ) -> NetworkRentParameters {
            NetworkRentParameters::try_new(
                rate,
                multiplier,
                persistent_ttl,
                warning,
                critical,
                ledgers_per_month,
            )
            .expect("test parameters should be valid")
        }

        #[test]
        fn calculates_monthly_cost_and_percentage_increase() {
            let entries = [
                ScenarioEntry::new("instance", 100, 500),
                ScenarioEntry::new("persistent:balance", 300, 250),
            ];
            let baseline = parameters(2.0, 1.0, 200, 100, 20, 1_000);
            let simulated = parameters(3.0, 2.0, 100, 100, 20, 1_000);

            let impact =
                evaluate_protocol_scenario(&entries, &baseline, &simulated).expect("scenario");
            assert_eq!(impact.baseline_monthly_upkeep_stroops, 4_000.0);
            assert_eq!(impact.simulated_monthly_upkeep_stroops, 24_000.0);
            assert_eq!(impact.monthly_upkeep_change_percent, Some(500.0));
            assert_eq!(impact.entries[0].baseline_monthly_upkeep_stroops, 1_000.0);
            assert_eq!(impact.entries[1].simulated_monthly_upkeep_stroops, 18_000.0);
        }

        #[test]
        fn longer_ttl_reduces_monthly_upkeep_without_changing_domain_defaults() {
            let source_thresholds = ThresholdConfig::default();
            let entries = [ScenarioEntry::new("persistent:key", 250, 500_000)];
            let baseline = parameters(4.0, 1.0, 100_000, 120_960, 34_560, 500_000);
            let simulated = parameters(4.0, 1.0, 200_000, 120_960, 34_560, 500_000);

            let impact =
                evaluate_protocol_scenario(&entries, &baseline, &simulated).expect("scenario");
            assert_eq!(impact.baseline_monthly_upkeep_stroops, 5_000.0);
            assert_eq!(impact.simulated_monthly_upkeep_stroops, 2_500.0);
            assert_eq!(impact.monthly_upkeep_change_percent, Some(-50.0));
            assert_eq!(ThresholdConfig::default(), source_thresholds);
        }

        #[test]
        fn uses_simulated_thresholds_to_identify_immediate_breaches() {
            let entries = [
                ScenarioEntry::new("warned", 10, 70),
                ScenarioEntry::new("critical", 10, 50),
                ScenarioEntry::new("archived", 10, 0),
                ScenarioEntry::new("safe", 10, 100),
            ];
            let baseline = parameters(1.0, 1.0, 500, 100, 20, 1_000);
            let simulated = parameters(1.0, 1.0, 500, 80, 60, 1_000);
            let impact =
                evaluate_protocol_scenario(&entries, &baseline, &simulated).expect("scenario");

            assert_eq!(impact.entries[0].simulated_status, RiskLevel::Warn);
            assert!(impact.entries[0].breaches_warning_threshold);
            assert!(!impact.entries[0].breaches_critical_threshold);
            assert_eq!(impact.entries[1].simulated_status, RiskLevel::Critical);
            assert!(impact.entries[1].breaches_warning_threshold);
            assert!(impact.entries[1].breaches_critical_threshold);
            assert_eq!(impact.entries[2].simulated_status, RiskLevel::Archived);
            assert!(impact.entries[2].breaches_critical_threshold);
            assert_eq!(impact.entries[3].simulated_status, RiskLevel::Ok);
            assert!(!impact.entries[3].breaches_warning_threshold);
        }

        #[test]
        fn handles_zero_baseline_cost_without_infinite_percentage() {
            let entries = [ScenarioEntry::new("persistent:key", 100, 500)];
            let baseline = parameters(0.0, 1.0, 100, 100, 20, 1_000);
            let no_cost = evaluate_protocol_scenario(&entries, &baseline, &baseline)
                .expect("zero cost scenario");
            assert_eq!(no_cost.monthly_upkeep_change_percent, Some(0.0));

            let nonzero = parameters(1.0, 1.0, 100, 100, 20, 1_000);
            let increase =
                evaluate_protocol_scenario(&entries, &baseline, &nonzero).expect("scenario");
            assert_eq!(increase.monthly_upkeep_change_percent, None);
        }

        #[test]
        fn rejects_invalid_parameter_overrides() {
            let invalid = NetworkRentParameters {
                fee_rate_per_byte: f64::NAN,
                ..parameters(1.0, 1.0, 100, 100, 20, 1_000)
            };
            let result = evaluate_protocol_scenario(&[], &invalid, &invalid);
            assert!(matches!(result, Err(ScenarioError::InvalidParameters(_))));

            assert!(NetworkRentParameters::try_new(1.0, 1.0, 100, 20, 20, 1_000).is_err());

            let baseline = parameters(0.0, 1.0, 1, 100, 20, 1);
            let simulated = parameters(1.0e300, 1.0, 1, 100, 20, 1);
            let oversized = [ScenarioEntry::new("oversized", usize::MAX, 100)];
            assert!(evaluate_protocol_scenario(&oversized, &baseline, &simulated).is_err());
        }

        #[test]
        fn use_case_maps_scan_entries_into_scenario_inputs() {
            let report = TTLReport {
                contract_id: "CA...".to_string(),
                network: "testnet".to_string(),
                latest_ledger: 100,
                ledger_velocity_seconds: 5.0,
                entries: vec![StorageEntryReport::new(
                    "persistent:ADMIN",
                    EntryType::PersistentData,
                    Durability::Persistent,
                    128,
                    150,
                    50,
                    250.0,
                    250.0 / 86_400.0,
                    RiskLevel::Ok,
                )],
                overall_status: RiskLevel::Ok,
                archival_recommendations: Vec::new(),
            };
            let parameters = parameters(1.0, 1.0, 100, 100, 20, 1_000);

            let impact = ProtocolScenarioUseCase
                .execute(&report, &parameters, &parameters)
                .expect("scenario use case");
            assert_eq!(impact.entries.len(), 1);
            assert_eq!(impact.entries[0].key_identifier, "persistent:ADMIN");
            assert_eq!(impact.entries[0].baseline_monthly_upkeep_stroops, 1_280.0);
        }
    }
}
