//! Contract executable upgrade detection and minimum-TTL rent impact.

use serde::{Deserialize, Serialize};

/// Default minimum persistent-entry TTL used for rent-obligation estimates.
pub const DEFAULT_MINIMUM_TTL_LEDGERS: u32 = 4_096;

/// Identifies the current WASM executable and its actual bytecode size.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContractCodeState {
    pub code_hash: [u8; 32],
    pub bytecode_size_bytes: u64,
}

/// Domain event emitted when a contract's WASM executable changes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContractUpgradeEvent {
    pub previous_code_hash: [u8; 32],
    pub new_code_hash: [u8; 32],
    pub ledger_height: u32,
    /// Signed byte delta; positive values indicate bytecode expansion.
    pub bytecode_size_delta: i128,
    /// Signed minimum-TTL obligation delta in byte-ledgers.
    pub minimum_ttl_rent_delta: i128,
}

/// Detects an executable hash change and estimates its minimum-TTL rent impact.
///
/// The rent obligation is reported in byte-ledgers because the current scan
/// source does not supply the network's protocol-configured fee rate.
pub fn detect_contract_upgrade(
    previous: Option<&ContractCodeState>,
    current: Option<&ContractCodeState>,
    ledger_height: u32,
    minimum_ttl_ledgers: u32,
) -> Option<ContractUpgradeEvent> {
    let (previous, current) = (previous?, current?);
    if previous.code_hash == current.code_hash {
        return None;
    }

    let bytecode_size_delta =
        i128::from(current.bytecode_size_bytes) - i128::from(previous.bytecode_size_bytes);

    Some(ContractUpgradeEvent {
        previous_code_hash: previous.code_hash,
        new_code_hash: current.code_hash,
        ledger_height,
        bytecode_size_delta,
        minimum_ttl_rent_delta: bytecode_size_delta * i128::from(minimum_ttl_ledgers),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code_state(hash: u8, size: u64) -> ContractCodeState {
        ContractCodeState {
            code_hash: [hash; 32],
            bytecode_size_bytes: size,
        }
    }

    #[test]
    fn detects_expansion_and_calculates_minimum_ttl_rent() {
        let previous = code_state(1, 1_000);
        let current = code_state(2, 1_500);

        let event = detect_contract_upgrade(Some(&previous), Some(&current), 123, 4_096)
            .expect("hash mutation should produce an event");

        assert_eq!(event.previous_code_hash, [1; 32]);
        assert_eq!(event.new_code_hash, [2; 32]);
        assert_eq!(event.ledger_height, 123);
        assert_eq!(event.bytecode_size_delta, 500);
        assert_eq!(event.minimum_ttl_rent_delta, 500 * 4_096);
    }

    #[test]
    fn code_contraction_reduces_minimum_ttl_rent() {
        let previous = code_state(1, 1_500);
        let current = code_state(2, 1_000);

        let event = detect_contract_upgrade(Some(&previous), Some(&current), 123, 4_096)
            .expect("hash mutation should produce an event");

        assert_eq!(event.bytecode_size_delta, -500);
        assert_eq!(event.minimum_ttl_rent_delta, -500 * 4_096);
    }

    #[test]
    fn does_not_emit_event_without_two_different_wasm_states() {
        let previous = code_state(1, 1_000);
        assert!(detect_contract_upgrade(None, Some(&previous), 123, 4_096).is_none());
        assert!(detect_contract_upgrade(Some(&previous), None, 123, 4_096).is_none());
        assert!(detect_contract_upgrade(Some(&previous), Some(&previous), 123, 4_096).is_none());
    }
}
