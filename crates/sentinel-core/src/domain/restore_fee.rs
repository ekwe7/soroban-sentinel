//! Protocol 20/21 storage resource and rent fees for RestoreFootprint.

use std::num::{NonZeroU32, NonZeroU64};

use super::{Durability, RentDebtCalculator, StorageEntry};

const BYTES_PER_KIB: u128 = 1024;
// Protocol 20/21 soroban-env-host fee accounting size for each TTL write.
const TTL_ENTRY_BYTES: u128 = 48;
const STROOPS_PER_XLM: u128 = 10_000_000;

/// Effective ledger fee parameters for the network and ledger being modeled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RestoreFeeCalculator {
    pub fee_write_1kb: u64,
    pub min_persistent_entry_ttl: NonZeroU32,
    pub fee_write_entry: u64,
    pub fee_read_1kb: u64,
    pub fee_read_entry: u64,
    pub persistent_rent_rate_denominator: NonZeroU64,
}

/// Storage-specific cost. Full transaction fees also depend on the envelope
/// size and the inclusion fee bid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoreFee {
    /// Footprint reads and writes plus TTL entry writes.
    pub base_restoration_fee_stroops: u128,
    /// Rent charged for the fresh minimum lifetime of the restored entries.
    pub initial_rent_stroops: u128,
    pub total_stroops: u128,
    /// Exact decimal representation with seven fractional digits.
    pub total_xlm: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RestoreFeeError {
    #[error("temporary storage cannot be restored")]
    TemporaryEntry,
    #[error("restoration requires the serialized byte size of each entry")]
    MissingSize,
    #[error("restoration exceeds the protocol's 32-bit resource counters")]
    ResourceOverflow,
}

impl RestoreFeeCalculator {
    /// Quote one archived entry with its full ledger-entry XDR byte size.
    pub fn calculate(&self, entry: &StorageEntry) -> Result<RestoreFee, RestoreFeeError> {
        self.calculate_batch(std::slice::from_ref(entry))
    }

    /// Quote one footprint of distinct, archived entries. Rent rounds per entry,
    /// while read, write, and TTL write byte fees round on batch totals.
    pub fn calculate_batch(&self, entries: &[StorageEntry]) -> Result<RestoreFee, RestoreFeeError> {
        let rent = RentDebtCalculator {
            write_fee_per_kib_stroops: self.fee_write_1kb,
            write_fee_per_entry_stroops: self.fee_write_entry,
            persistent_rent_rate_denominator: self.persistent_rent_rate_denominator,
            min_persistent_ttl: self.min_persistent_entry_ttl,
        };
        let mut total_bytes = 0_u32;
        let mut initial_rent_stroops = 0;
        for entry in entries {
            if entry.durability() == Durability::Temporary {
                return Err(RestoreFeeError::TemporaryEntry);
            }
            let size =
                u32::try_from(entry.byte_size()).map_err(|_| RestoreFeeError::ResourceOverflow)?;
            if size == 0 {
                return Err(RestoreFeeError::MissingSize);
            }
            total_bytes = total_bytes
                .checked_add(size)
                .ok_or(RestoreFeeError::ResourceOverflow)?;
            initial_rent_stroops += rent.minimum_ttl_rent_stroops(size);
        }

        let count = entries.len() as u128;
        let bytes = u128::from(total_bytes);
        let write_rate = u128::from(self.fee_write_1kb);
        let base_restoration_fee_stroops = count
            * (u128::from(self.fee_read_entry) + 2 * u128::from(self.fee_write_entry))
            + (bytes * u128::from(self.fee_read_1kb)).div_ceil(BYTES_PER_KIB)
            + (bytes * write_rate).div_ceil(BYTES_PER_KIB)
            + (count * TTL_ENTRY_BYTES * write_rate).div_ceil(BYTES_PER_KIB);
        let total_stroops = base_restoration_fee_stroops + initial_rent_stroops;
        Ok(RestoreFee {
            base_restoration_fee_stroops,
            initial_rent_stroops,
            total_stroops,
            total_xlm: format!(
                "{}.{:07}",
                total_stroops / STROOPS_PER_XLM,
                total_stroops % STROOPS_PER_XLM
            ),
        })
    }
}
