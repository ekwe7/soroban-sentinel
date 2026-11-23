//! Restoration rent and modeled arrears for expired Soroban storage.

use std::num::{NonZeroU32, NonZeroU64};

use serde::{Deserialize, Serialize};

use super::Durability;

const BYTES_PER_KIB: u128 = 1024;
const TTL_ENTRY_BYTES: u128 = 68;

/// CAP-0046-07 rent parameters for the network and ledger being modeled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RentDebtCalculator {
    /// Effective write fee per KiB used by the CAP-0046 rent formula.
    pub write_fee_per_kib_stroops: u64,
    pub write_fee_per_entry_stroops: u64,
    pub persistent_rent_rate_denominator: NonZeroU64,
    /// Network minimum persistent TTL granted on restoration, including the restore ledger.
    pub min_persistent_ttl: NonZeroU32,
}

/// Per-entry breakdown in whole stroops.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RentDebt {
    pub expired_ledgers: u32,
    /// Hypothetical back rent at the supplied rate. CAP-0046 does not collect this amount.
    pub arrears_recovery_fee_stroops: u128,
    /// Rent for the new lifetime granted by restoration.
    pub new_lifetime_extension_fee_stroops: u128,
    pub ttl_write_fee_stroops: u128,
    /// CAP-0046 rent and TTL write subtotal. Transaction inclusion and footprint fees are separate.
    pub restoration_rent_stroops: u128,
    /// Planning capital for restoration rent, TTL write, and hypothetical arrears.
    pub modeled_total_stroops: u128,
}

impl RentDebtCalculator {
    /// Estimate rent for a new minimum persistent TTL in whole stroops.
    pub fn minimum_ttl_rent_stroops(&self, byte_size: u32) -> u128 {
        self.rent(byte_size, self.min_persistent_ttl.get())
    }

    /// Compute costs for one expired entry with a known original TTL and serialized size.
    /// The original live-until ledger is inclusive. Zero is the scan's missing-TTL sentinel.
    pub fn calculate(
        &self,
        durability: Durability,
        byte_size: u32,
        live_until_ledger: u32,
        latest_ledger: u32,
    ) -> Option<RentDebt> {
        if durability == Durability::Temporary
            || live_until_ledger == 0
            || latest_ledger <= live_until_ledger
            || byte_size == 0
        {
            return None;
        }

        let expired_ledgers = latest_ledger - live_until_ledger;
        let arrears_recovery_fee_stroops = self.rent(byte_size, expired_ledgers);
        let new_lifetime_extension_fee_stroops =
            self.rent(byte_size, self.min_persistent_ttl.get());
        let ttl_write_fee_stroops = u128::from(self.write_fee_per_entry_stroops)
            + (TTL_ENTRY_BYTES * u128::from(self.write_fee_per_kib_stroops))
                .div_ceil(BYTES_PER_KIB);
        let restoration_rent_stroops = new_lifetime_extension_fee_stroops + ttl_write_fee_stroops;

        Some(RentDebt {
            expired_ledgers,
            arrears_recovery_fee_stroops,
            new_lifetime_extension_fee_stroops,
            ttl_write_fee_stroops,
            restoration_rent_stroops,
            modeled_total_stroops: arrears_recovery_fee_stroops + restoration_rent_stroops,
        })
    }

    fn rent(&self, byte_size: u32, ledgers: u32) -> u128 {
        let numerator = u128::from(byte_size)
            * u128::from(ledgers)
            * u128::from(self.write_fee_per_kib_stroops);
        numerator.div_ceil(BYTES_PER_KIB * u128::from(self.persistent_rent_rate_denominator.get()))
    }
}
