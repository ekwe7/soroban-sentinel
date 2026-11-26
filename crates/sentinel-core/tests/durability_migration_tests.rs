use sentinel_core::domain::{
    Durability, DurabilityMigrationAnalysis, MigrationRequest, NetworkLimits, RentRates,
};
use std::num::{NonZeroU32, NonZeroU64};

fn rates() -> RentRates {
    RentRates {
        fee_per_rent_kib_stroops: 1000,
        persistent_denominator: NonZeroU64::new(10).unwrap(),
        temporary_denominator: NonZeroU64::new(100).unwrap(),
        ledgers_per_year: NonZeroU32::new(365).unwrap(),
        code_rent_discount: NonZeroU32::new(1).unwrap(),
    }
}
fn limits() -> NetworkLimits {
    NetworkLimits {
        max_entry_ttl: 1000,
        min_persistent_ttl: 10,
        min_temporary_ttl: 1,
        max_contract_data_entry_bytes: 65536,
        protocol_version: 22,
    }
}
fn request() -> MigrationRequest {
    MigrationRequest {
        byte_size: 1024,
        from: Durability::Persistent,
        to: Durability::Temporary,
        horizon_ledgers: 500,
        replaceable: false,
    }
}

#[test]
fn annual_delta_matches_protocol_byte_ledger_formula_in_both_directions() {
    // Protocol rent_fee_for_size_and_ledgers rounds AFTER dividing the full
    // byte-ledger product by (1024 * durability denominator).
    let result = DurabilityMigrationAnalysis::analyze(request(), &rates(), &limits()).unwrap();
    assert_eq!(result.source_annual_rent_stroops, 36500);
    assert_eq!(result.target_annual_rent_stroops, 3650);
    assert_eq!(result.annual_rent_delta_stroops, -32850);
    assert_eq!(result.annual_rent_delta_xlm, -0.003285);
    assert!(result
        .warnings
        .iter()
        .any(|warning| warning.contains("non-replaceable")));
    let reverse = DurabilityMigrationAnalysis::analyze(
        MigrationRequest {
            from: Durability::Temporary,
            to: Durability::Persistent,
            ..request()
        },
        &rates(),
        &limits(),
    )
    .unwrap();
    assert_eq!(reverse.annual_rent_delta_stroops, 32850);
    assert!(reverse.warnings.is_empty());
}

#[test]
fn fractional_kib_is_rounded_once_per_renewal_and_partial_year() {
    let rates = RentRates {
        fee_per_rent_kib_stroops: 1,
        ledgers_per_year: NonZeroU32::new(101).unwrap(),
        ..rates()
    };
    let result = DurabilityMigrationAnalysis::analyze(
        MigrationRequest {
            byte_size: 1,
            horizon_ledgers: 50,
            ..request()
        },
        &rates,
        &limits(),
    )
    .unwrap();
    // Two 50-ledger renewals and a 1-ledger tail each cost ceil(fraction)=1.
    assert_eq!(result.source_annual_rent_stroops, 3);
    assert_eq!(result.target_annual_rent_stroops, 3);
    assert_eq!(
        rates.entry_rent(1025, 10, Durability::Persistent).unwrap(),
        2
    );
}

#[test]
fn network_maximum_and_minimum_horizons_are_explicit() {
    let result = DurabilityMigrationAnalysis::analyze(
        MigrationRequest {
            horizon_ledgers: 2000,
            ..request()
        },
        &rates(),
        &limits(),
    )
    .unwrap();
    assert_eq!(result.target_horizon_ledgers, 999);
    assert!(result
        .warnings
        .iter()
        .any(|warning| warning.contains("max_entry_ttl")));
    let result = DurabilityMigrationAnalysis::analyze(
        MigrationRequest {
            from: Durability::Temporary,
            to: Durability::Persistent,
            horizon_ledgers: 1,
            ..request()
        },
        &rates(),
        &limits(),
    )
    .unwrap();
    assert_eq!(result.source_horizon_ledgers, 1);
    assert_eq!(result.target_horizon_ledgers, 9);
    assert!(DurabilityMigrationAnalysis::analyze(
        MigrationRequest {
            horizon_ledgers: 0,
            ..request()
        },
        &rates(),
        &limits()
    )
    .is_err());
    assert!(DurabilityMigrationAnalysis::analyze(
        request(),
        &rates(),
        &NetworkLimits {
            max_entry_ttl: 0,
            ..limits()
        }
    )
    .is_err());
    assert!(DurabilityMigrationAnalysis::analyze(
        MigrationRequest {
            to: Durability::Persistent,
            ..request()
        },
        &rates(),
        &limits()
    )
    .is_err());
}

#[test]
fn large_products_use_wide_integer_arithmetic() {
    let rates = RentRates {
        fee_per_rent_kib_stroops: u64::MAX,
        ledgers_per_year: NonZeroU32::new(u32::MAX).unwrap(),
        ..rates()
    };
    let result = DurabilityMigrationAnalysis::analyze(
        MigrationRequest {
            byte_size: u32::MAX,
            ..request()
        },
        &rates,
        &limits(),
    )
    .unwrap();
    assert!(result.source_annual_rent_stroops > u128::from(u64::MAX));
}
