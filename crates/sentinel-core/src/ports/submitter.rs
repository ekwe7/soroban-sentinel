//! Submission boundary for already-signed transaction envelopes.

use std::time::Duration;

use async_trait::async_trait;

use crate::SentinelResult;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubmissionOutcome {
    Confirmed { ledger: u32, tx_hash: String },
    Failed { tx_hash: String, result_xdr: String },
    TimedOut { tx_hash: String },
}

#[derive(Debug, Clone, Copy)]
pub struct SubmissionPolicy {
    /// Deadline for the send phase, including explicit retries.
    pub send_timeout: Duration,
    /// Deadline for the lookup phase, including in-flight requests.
    pub poll_timeout: Duration,
    pub poll_interval: Duration,
}

impl Default for SubmissionPolicy {
    fn default() -> Self {
        Self {
            send_timeout: Duration::from_secs(30),
            poll_timeout: Duration::from_secs(60),
            poll_interval: Duration::from_secs(1),
        }
    }
}

#[async_trait]
pub trait TransactionSubmitter: Send + Sync {
    async fn submit(
        &self,
        signed_envelope: &str,
        network_passphrase: &str,
        policy: SubmissionPolicy,
    ) -> SentinelResult<SubmissionOutcome>;
}

/// Returns a configured response without contacting the network.
pub struct MockTransactionSubmitter(pub SentinelResult<SubmissionOutcome>);

#[async_trait]
impl TransactionSubmitter for MockTransactionSubmitter {
    async fn submit(
        &self,
        _signed_envelope: &str,
        _network_passphrase: &str,
        _policy: SubmissionPolicy,
    ) -> SentinelResult<SubmissionOutcome> {
        self.0.clone()
    }
}
