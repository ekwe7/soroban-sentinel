//! Contract executable mutation detection and minimum-TTL rent deltas.

use serde::{Deserialize, Serialize};

/// Baseline WASM executable details from a previous contract scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContractUpgradeBaseline {
    pub code_hash: [u8; 32],
    pub bytecode_size: u32,
}

impl ContractUpgradeBaseline {
    pub fn new(code_hash: [u8; 32], bytecode_size: u32) -> Self {
        Self {
            code_hash,
            bytecode_size,
        }
    }
}

/// Event emitted when a scan observes a different WASM executable than its baseline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContractUpgradeEvent {
    pub previous_code_hash: String,
    pub new_code_hash: String,
    pub ledger_height: u32,
    /// Positive values indicate bytecode expansion; negative values indicate shrinkage.
    pub bytecode_size_delta: i64,
    /// Positive values indicate additional minimum-TTL rent in stroops.
    pub minimum_ttl_rent_delta_stroops: i128,
}

/// Returns an upgrade event when the current WASM hash differs from the baseline.
pub fn detect_contract_upgrade(
    baseline: ContractUpgradeBaseline,
    current_code_hash: [u8; 32],
    current_bytecode_size: u32,
    ledger_height: u32,
    minimum_ttl_rent_delta_stroops: i128,
) -> Option<ContractUpgradeEvent> {
    (baseline.code_hash != current_code_hash).then(|| ContractUpgradeEvent {
        previous_code_hash: format_contract_code_hash(&baseline.code_hash),
        new_code_hash: format_contract_code_hash(&current_code_hash),
        ledger_height,
        bytecode_size_delta: i64::from(current_bytecode_size) - i64::from(baseline.bytecode_size),
        minimum_ttl_rent_delta_stroops,
    })
}

pub fn format_contract_code_hash(hash: &[u8; 32]) -> String {
    use std::fmt::Write;

    let mut encoded = String::with_capacity(hash.len() * 2);
    for byte in hash {
        write!(&mut encoded, "{byte:02x}").expect("writing to a String cannot fail");
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_hash_mutation_and_size_and_rent_deltas() {
        let baseline = ContractUpgradeBaseline::new([1; 32], 1_000);
        let event = detect_contract_upgrade(baseline, [2; 32], 1_500, 42, 3_250).unwrap();

        assert_eq!(event.previous_code_hash, "01".repeat(32));
        assert_eq!(event.new_code_hash, "02".repeat(32));
        assert_eq!(event.ledger_height, 42);
        assert_eq!(event.bytecode_size_delta, 500);
        assert_eq!(event.minimum_ttl_rent_delta_stroops, 3_250);
    }

    #[test]
    fn ignores_unchanged_executable_hash() {
        let baseline = ContractUpgradeBaseline::new([7; 32], 1_000);
        assert!(detect_contract_upgrade(baseline, [7; 32], 1_200, 42, 0).is_none());
    }

    #[test]
    fn supports_shrinkage_and_negative_rent_deltas() {
        let baseline = ContractUpgradeBaseline::new([1; 32], 1_500);
        let event = detect_contract_upgrade(baseline, [2; 32], 1_000, 42, -3_250).unwrap();

        assert_eq!(event.bytecode_size_delta, -500);
        assert_eq!(event.minimum_ttl_rent_delta_stroops, -3_250);
    }
}
