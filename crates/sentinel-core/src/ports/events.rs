//! Abstract port for reading Soroban contract event streams.

use async_trait::async_trait;

use crate::domain::events::ContractEvent;
use crate::error::SentinelResult;

/// Port interface for querying contract and State Archival events.
///
/// Implementations translate an RPC event source into parsed [`ContractEvent`]
/// values. This port intentionally covers bounded, one-shot reads only; live
/// WebSocket streaming and daemon loops are outside its scope.
#[async_trait]
pub trait EventStreamReader: Send + Sync {
    /// Retrieves events for a contract starting at `start_ledger`.
    ///
    /// Implementations should follow RPC pagination cursors and return events
    /// in ledger order without duplicates.
    async fn get_contract_events(
        &self,
        contract_id: &str,
        start_ledger: u32,
    ) -> SentinelResult<Vec<ContractEvent>>;
}
