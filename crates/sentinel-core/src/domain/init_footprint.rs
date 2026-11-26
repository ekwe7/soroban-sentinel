//! Approximate storage footprint and rent for a newly initialized contract.
//!
//! Sizes are based on XDR-encoded constructor values plus fixed ledger-entry
//! framing estimates. Rent uses a deterministic baseline of 1 stroop per KiB
//! per ledger with a 1:1 persistent denominator; live network fee parameters
//! can differ, so these figures are planning estimates rather than fee quotes.

use serde::{Deserialize, Serialize};
use stellar_xdr::curr::{Limits, ScVal, WriteXdr};

pub const LEDGERS_PER_DAY: u32 = 17_280;
pub const STROOPS_PER_XLM: f64 = 10_000_000.0;
const CODE_ENTRY_OVERHEAD_BYTES: usize = 48;
const INSTANCE_BASE_OVERHEAD_BYTES: usize = 128;
const INSTANCE_MAP_ENTRY_OVERHEAD_BYTES: usize = 32;

/// Predicted byte footprint and baseline rent reserves for contract creation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InitialFootprintEstimate {
    /// Mandatory contract WASM code entry, including estimated ledger framing.
    pub contract_code_bytes: usize,
    /// Contract instance plus its initial storage map and values.
    pub contract_instance_bytes: usize,
    /// Aggregate predicted storage footprint.
    pub total_footprint_bytes: usize,
    pub rent_30_day_stroops: u128,
    pub rent_30_day_xlm: f64,
    pub rent_90_day_stroops: u128,
    pub rent_90_day_xlm: f64,
}

/// Predict initial contract storage and baseline 30/90-day persistent rent.
///
/// `initial_keys` are the constructor-provided values stored in the instance
/// data map. This uses their XDR sizes and estimates one map key per value.
pub fn predict_initial_footprint(
    contract_wasm_size: usize,
    initial_keys: &[ScVal],
) -> InitialFootprintEstimate {
    let contract_code_bytes = contract_wasm_size.saturating_add(CODE_ENTRY_OVERHEAD_BYTES);
    let values_bytes = initial_keys.iter().fold(0usize, |sum, value| {
        sum.saturating_add(value.to_xdr(Limits::none()).map_or(0, |xdr| xdr.len()))
    });
    // A storage map key is represented by a ScVal too; use the encoded value
    // size as a conservative proxy when the constructor signature is unknown.
    let map_bytes = initial_keys.iter().fold(0usize, |sum, value| {
        let size = value.to_xdr(Limits::none()).map_or(0, |xdr| xdr.len());
        sum.saturating_add(size.saturating_mul(2))
            .saturating_add(INSTANCE_MAP_ENTRY_OVERHEAD_BYTES)
    });
    let contract_instance_bytes = INSTANCE_BASE_OVERHEAD_BYTES
        .saturating_add(values_bytes)
        .saturating_add(map_bytes);
    let total_footprint_bytes = contract_code_bytes.saturating_add(contract_instance_bytes);

    let rent = |ledgers: u32| -> u128 {
        // ceil(bytes * ledgers / 1024), in baseline stroops.
        (total_footprint_bytes as u128)
            .saturating_mul(u128::from(ledgers))
            .div_ceil(1024)
    };
    let rent_30_day_stroops = rent(30 * LEDGERS_PER_DAY);
    let rent_90_day_stroops = rent(90 * LEDGERS_PER_DAY);

    InitialFootprintEstimate {
        contract_code_bytes,
        contract_instance_bytes,
        total_footprint_bytes,
        rent_30_day_stroops,
        rent_30_day_xlm: rent_30_day_stroops as f64 / STROOPS_PER_XLM,
        rent_90_day_stroops,
        rent_90_day_xlm: rent_90_day_stroops as f64 / STROOPS_PER_XLM,
    }
}
