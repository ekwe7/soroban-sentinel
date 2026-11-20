//! Domain data for Soroban contract WASM code inspection.

use serde::{Deserialize, Serialize};

/// Default Soroban contract WASM size limit used for advisory checks.
pub const DEFAULT_MAX_WASM_SIZE_BYTES: usize = 64 * 1024;

/// A parsed WASM section and its encoded size in the module.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WasmSectionSize {
    pub id: u8,
    pub name: String,
    pub size_bytes: usize,
}

/// Soroban host interface version embedded in `contractenvmetav0`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SorobanInterfaceVersion {
    pub protocol: u32,
    pub pre_release: u32,
}

/// Metadata extracted from a ContractCode entry's WASM bytecode.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WasmCodeInspection {
    /// Raw WASM module length, excluding the enclosing ledger-entry XDR.
    pub wasm_size_bytes: usize,
    /// Encoded sizes of the module's sections.
    pub section_sizes: Vec<WasmSectionSize>,
    /// Combined payload size of `contractspecv0` sections, if present.
    pub contract_spec_size_bytes: Option<usize>,
    /// Soroban host interface version, if present and valid.
    pub interface_version: Option<SorobanInterfaceVersion>,
    /// Soroban SDK version from the `rssdkver` contract metadata entry.
    pub sdk_version: Option<String>,
    /// Build identifier from contract metadata, when provided by the contract.
    pub build_id: Option<String>,
    /// Code byte-ledger exposure through its current TTL.
    pub byte_ledger_exposure: u128,
    /// Remaining ledgers until the code entry is archived.
    pub code_remaining_ledgers: i64,
    /// Remaining ledgers until the associated instance entry is archived.
    pub instance_remaining_ledgers: i64,
    /// Difference between code and instance remaining TTL.
    pub ttl_delta_from_instance_ledgers: i64,
    /// Whether code TTL is at or below the configured critical threshold.
    pub nearing_archival: bool,
    /// Whether the module exceeds the advisory size limit.
    pub exceeds_size_limit: bool,
}

impl WasmCodeInspection {
    /// Constructs inspection output using decoded module and ledger data.
    pub fn new(
        wasm_size_bytes: usize,
        section_sizes: Vec<WasmSectionSize>,
        contract_spec_size_bytes: Option<usize>,
        interface_version: Option<SorobanInterfaceVersion>,
        sdk_version: Option<String>,
        build_id: Option<String>,
        remaining_ledgers: i64,
        instance_remaining_ledgers: i64,
        critical_ledgers: u32,
        max_wasm_size_bytes: usize,
    ) -> Self {
        let byte_ledger_exposure = wasm_size_bytes as u128 * remaining_ledgers.max(0) as u128;
        Self {
            wasm_size_bytes,
            section_sizes,
            contract_spec_size_bytes,
            interface_version,
            sdk_version,
            build_id,
            byte_ledger_exposure,
            code_remaining_ledgers: remaining_ledgers,
            instance_remaining_ledgers,
            ttl_delta_from_instance_ledgers: remaining_ledgers
                .saturating_sub(instance_remaining_ledgers),
            nearing_archival: remaining_ledgers <= critical_ledgers as i64,
            exceeds_size_limit: wasm_size_bytes > max_wasm_size_bytes,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_exposure_is_zero_for_archived_code_and_checks_size_limit() {
        let inspection = WasmCodeInspection::new(
            DEFAULT_MAX_WASM_SIZE_BYTES + 1,
            Vec::new(),
            None,
            None,
            None,
            None,
            -1,
            -1,
            34_560,
            DEFAULT_MAX_WASM_SIZE_BYTES,
        );

        assert_eq!(inspection.byte_ledger_exposure, 0);
        assert!(inspection.exceeds_size_limit);
    }
}
