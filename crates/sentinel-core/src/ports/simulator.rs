use async_trait::async_trait;

use crate::error::SentinelResult;

/// Result returned by a Soroban transaction simulation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RentEstimate {
    pub min_resource_fee_stroops: u64,
    pub latest_ledger: u32,
    pub cpu_instructions: u64,
    pub memory_bytes: u64,
}

/// Port for simulating unsigned Soroban transactions.
#[async_trait]
pub trait SimulationEngine: Send + Sync {
    async fn simulate_transaction(
        &self,
        transaction_xdr_base64: &str,
        latest_ledger: u32,
    ) -> SentinelResult<RentEstimate>;
}

#[derive(Debug, Clone, Default)]
pub struct MockSimulationEngine {
    result: Option<RentEstimate>,
    error: Option<crate::error::SentinelError>,
}

impl MockSimulationEngine {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn with_result(mut self, result: RentEstimate) -> Self {
        self.result = Some(result);
        self
    }
    pub fn with_error(mut self, error: crate::error::SentinelError) -> Self {
        self.error = Some(error);
        self
    }
}

#[async_trait]
impl SimulationEngine for MockSimulationEngine {
    async fn simulate_transaction(
        &self,
        _transaction_xdr_base64: &str,
        _latest_ledger: u32,
    ) -> SentinelResult<RentEstimate> {
        if let Some(error) = &self.error {
            return Err(error.clone());
        }
        self.result
            .clone()
            .ok_or_else(|| crate::error::SentinelError::RpcResponseError {
                code: -1,
                message: "mock simulation result not configured".to_owned(),
            })
    }
}
