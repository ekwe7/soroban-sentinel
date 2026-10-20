//! In-memory mock implementation of `LedgerReader` for offline unit testing.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use crate::error::{SentinelError, SentinelResult};
use crate::ports::reader::{LatestLedgerInfo, LedgerReader, RawLedgerEntry};

/// In-memory mock implementation of [`LedgerReader`].
#[derive(Debug, Clone, Default)]
pub struct MockLedgerReader {
    latest_ledger: Arc<Mutex<Option<LatestLedgerInfo>>>,
    entries: Arc<Mutex<Vec<RawLedgerEntry>>>,
    injected_error: Arc<Mutex<Option<SentinelError>>>,
}

impl MockLedgerReader {
    /// Create a new empty `MockLedgerReader`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the stubbed latest ledger response.
    pub fn with_latest_ledger(self, sequence: u32, protocol_version: u32) -> Self {
        *self.latest_ledger.lock().unwrap() =
            Some(LatestLedgerInfo::new(sequence, protocol_version));
        self
    }

    /// Add a single raw ledger entry to the mock storage.
    pub fn with_entry(self, entry: RawLedgerEntry) -> Self {
        self.entries.lock().unwrap().push(entry);
        self
    }

    /// Add multiple raw ledger entries to the mock storage.
    pub fn with_entries(self, entries: Vec<RawLedgerEntry>) -> Self {
        self.entries.lock().unwrap().extend(entries);
        self
    }

    /// Injects an error that will be returned by all calls.
    pub fn with_error(self, error: SentinelError) -> Self {
        *self.injected_error.lock().unwrap() = Some(error);
        self
    }
}

#[async_trait]
impl LedgerReader for MockLedgerReader {
    async fn get_latest_ledger(&self) -> SentinelResult<LatestLedgerInfo> {
        if let Some(err) = self.injected_error.lock().unwrap().clone() {
            return Err(err);
        }

        let guard = *self.latest_ledger.lock().unwrap();
        guard.ok_or_else(|| SentinelError::RpcResponseError {
            code: -1,
            message: "Mock latest ledger not configured".to_string(),
        })
    }

    async fn get_ledger_entries(
        &self,
        keys_xdr_base64: &[String],
    ) -> SentinelResult<Vec<RawLedgerEntry>> {
        if let Some(err) = self.injected_error.lock().unwrap().clone() {
            return Err(err);
        }

        let guard = self.entries.lock().unwrap();
        let matched = guard
            .iter()
            .filter(|entry| keys_xdr_base64.contains(&entry.key_xdr_base64))
            .cloned()
            .collect();
        Ok(matched)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_mock_reader_latest_ledger() {
        let mock = MockLedgerReader::new().with_latest_ledger(5_432_100, 21);

        let info = mock.get_latest_ledger().await.unwrap();
        assert_eq!(info.sequence, 5_432_100);
        assert_eq!(info.protocol_version, 21);
    }

    #[tokio::test]
    async fn test_mock_reader_unconfigured_latest_ledger_error() {
        let mock = MockLedgerReader::new();
        let result = mock.get_latest_ledger().await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_mock_reader_filtered_entries() {
        let entry1 = RawLedgerEntry::new("key_1", "val_1", 6_000_000, 5_000_000);
        let entry2 = RawLedgerEntry::new("key_2", "val_2", 6_500_000, 5_100_000);
        let entry3 = RawLedgerEntry::new("key_3", "val_3", 7_000_000, 5_200_000);

        let mock =
            MockLedgerReader::new().with_entries(vec![entry1.clone(), entry2.clone(), entry3]);

        let queried = mock
            .get_ledger_entries(&["key_1".to_string(), "key_2".to_string()])
            .await
            .unwrap();

        assert_eq!(queried.len(), 2);
        assert_eq!(queried[0], entry1);
        assert_eq!(queried[1], entry2);
    }

    #[tokio::test]
    async fn test_mock_reader_injected_error() {
        let mock = MockLedgerReader::new().with_error(SentinelError::RpcConnectionTimeout(
            "mock timeout".to_string(),
        ));

        let res_latest = mock.get_latest_ledger().await;
        assert!(matches!(
            res_latest,
            Err(SentinelError::RpcConnectionTimeout(_))
        ));

        let res_entries = mock.get_ledger_entries(&["any_key".to_string()]).await;
        assert!(matches!(
            res_entries,
            Err(SentinelError::RpcConnectionTimeout(_))
        ));
    }
}
