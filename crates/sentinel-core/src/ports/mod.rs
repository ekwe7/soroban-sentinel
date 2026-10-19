//! Abstract port interfaces and mock adapters for Soroban Sentinel.

pub mod mock;
pub mod reader;

pub use mock::MockLedgerReader;
pub use reader::{LatestLedgerInfo, LedgerReader, RawLedgerEntry};
