//! Abstract port interfaces and mock adapters for Soroban Sentinel.

pub(crate) mod cache;
mod events;
pub mod mock;
pub mod notifier;
pub mod rate_limit;
pub mod reader;
pub mod simulator;

pub use cache::{CacheStats, CachedLedgerReader};
pub use events::EventStreamReader;
pub use mock::MockLedgerReader;
pub use notifier::{AlertKind, AlertMinLevel, AlertNotifier};
pub use rate_limit::{
    RateLimitConfigError, RateLimitedLedgerReader, RateLimiterConfig,
    DEFAULT_RATE_LIMIT_BURST_SIZE, DEFAULT_RATE_LIMIT_RPS,
};
pub use reader::{LatestLedgerInfo, LedgerReader, RawLedgerEntry, ReaderHealth};
pub use simulator::{MockSimulationEngine, RentEstimate, SimulationEngine};
pub mod submitter;
pub use submitter::{
    MockTransactionSubmitter, SubmissionOutcome, SubmissionPolicy, TransactionSubmitter,
};
