//! Deterministic JSON snapshot recording and offline replay for LedgerReader.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::error::{SentinelError, SentinelResult};
use crate::ports::reader::{LatestLedgerInfo, LedgerReader, RawLedgerEntry};

pub const SNAPSHOT_FORMAT_VERSION: u32 = 1;
pub const MAX_SNAPSHOT_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotHeader {
    pub format_version: u32,
    pub network_passphrase: String,
    pub protocol_version: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SnapshotData {
    pub latest_ledger: LatestLedgerInfo,
    #[serde(default)]
    pub ledger_close_velocity_seconds: Option<f64>,
    pub entries: Vec<RawLedgerEntry>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SentinelSnapshot {
    pub header: SnapshotHeader,
    pub data: SnapshotData,
}

impl SentinelSnapshot {
    pub fn write_deterministic(&self, path: &Path) -> SentinelResult<()> {
        let mut snapshot = self.clone();
        snapshot.data.entries.sort_by(|a, b| {
            (&a.key_xdr_base64, &a.entry_xdr_base64).cmp(&(&b.key_xdr_base64, &b.entry_xdr_base64))
        });
        let bytes = serde_json::to_vec(&snapshot).map_err(|e| {
            SentinelError::SerializationError(format!("Failed to encode snapshot: {e}"))
        })?;
        std::fs::write(path, bytes).map_err(|e| {
            SentinelError::SnapshotError(format!(
                "Failed to write snapshot '{}': {e}",
                path.display()
            ))
        })
    }

    pub fn read(path: &Path, network_passphrase: &str) -> SentinelResult<Self> {
        let metadata = std::fs::metadata(path).map_err(|e| {
            SentinelError::SnapshotError(format!(
                "Failed to read snapshot '{}': {e}",
                path.display()
            ))
        })?;
        if metadata.len() as usize > MAX_SNAPSHOT_BYTES {
            return Err(SentinelError::SnapshotError(format!(
                "Snapshot exceeds maximum allowed size of {MAX_SNAPSHOT_BYTES} bytes"
            )));
        }
        let bytes = std::fs::read(path).map_err(|e| {
            SentinelError::SnapshotError(format!(
                "Failed to read snapshot '{}': {e}",
                path.display()
            ))
        })?;
        let snapshot: SentinelSnapshot = serde_json::from_slice(&bytes).map_err(|e| {
            SentinelError::SnapshotError(format!("Corrupted or truncated snapshot: {e}"))
        })?;
        if snapshot.header.format_version != SNAPSHOT_FORMAT_VERSION {
            return Err(SentinelError::SnapshotError(format!(
                "Unsupported snapshot format version {}; expected {SNAPSHOT_FORMAT_VERSION}",
                snapshot.header.format_version
            )));
        }
        if snapshot.header.network_passphrase != network_passphrase {
            return Err(SentinelError::SnapshotError(format!(
                "Snapshot network passphrase mismatch: expected '{}', found '{}'",
                network_passphrase, snapshot.header.network_passphrase
            )));
        }
        if snapshot.data.entries.is_empty() {
            return Err(SentinelError::SnapshotError(
                "Snapshot contains no ledger entries".to_string(),
            ));
        }
        Ok(snapshot)
    }
}

fn snapshot_entries(snapshot: &SentinelSnapshot) -> BTreeMap<String, RawLedgerEntry> {
    snapshot
        .data
        .entries
        .iter()
        .map(|entry| (entry.key_xdr_base64.clone(), entry.clone()))
        .collect()
}

#[derive(Debug, Clone)]
pub struct ReplayReader {
    snapshot: SentinelSnapshot,
    entries: BTreeMap<String, RawLedgerEntry>,
}

impl ReplayReader {
    pub fn replay(
        path: impl Into<PathBuf>,
        network_passphrase: impl Into<String>,
    ) -> SentinelResult<Self> {
        let path = path.into();
        let passphrase = network_passphrase.into();
        let snapshot = SentinelSnapshot::read(&path, &passphrase)?;
        let entries = snapshot_entries(&snapshot);
        Ok(ReplayReader { snapshot, entries })
    }
}

#[async_trait]
impl LedgerReader for ReplayReader {
    async fn get_latest_ledger(&self) -> SentinelResult<LatestLedgerInfo> {
        Ok(self.snapshot.data.latest_ledger)
    }

    async fn get_ledger_entries(
        &self,
        keys_xdr_base64: &[String],
    ) -> SentinelResult<Vec<RawLedgerEntry>> {
        Ok(keys_xdr_base64
            .iter()
            .filter_map(|key| self.entries.get(key).cloned())
            .collect())
    }

    async fn get_ledger_close_velocity(&self, _latest_ledger: u32) -> SentinelResult<Option<f64>> {
        Ok(self.snapshot.data.ledger_close_velocity_seconds)
    }
}

enum SnapshotLedgerSource {
    Recording {
        inner: Box<dyn LedgerReader>,
        path: PathBuf,
        network_passphrase: String,
        state: Arc<RwLock<BTreeMap<String, RawLedgerEntry>>>,
        latest: Arc<RwLock<Option<LatestLedgerInfo>>>,
        velocity: Arc<RwLock<Option<f64>>>,
    },
    Replay(ReplayReader),
}

/// Records live `LedgerReader` responses to a deterministic snapshot, or replays
/// a previously recorded snapshot without any network access.
pub struct SnapshotLedgerReader {
    source: SnapshotLedgerSource,
}

impl SnapshotLedgerReader {
    /// Wraps a live reader and writes every observed query result to `path`.
    pub fn recording(
        inner: impl LedgerReader + 'static,
        path: impl Into<PathBuf>,
        network_passphrase: impl Into<String>,
    ) -> Self {
        Self {
            source: SnapshotLedgerSource::Recording {
                inner: Box::new(inner),
                path: path.into(),
                network_passphrase: network_passphrase.into(),
                state: Arc::new(RwLock::new(BTreeMap::new())),
                latest: Arc::new(RwLock::new(None)),
                velocity: Arc::new(RwLock::new(None)),
            },
        }
    }

    /// Opens a snapshot for deterministic offline replay.
    pub fn replay(
        path: impl Into<PathBuf>,
        network_passphrase: impl Into<String>,
    ) -> SentinelResult<Self> {
        Ok(Self {
            source: SnapshotLedgerSource::Replay(ReplayReader::replay(path, network_passphrase)?),
        })
    }
}

async fn persist_recording(
    path: &Path,
    network_passphrase: &str,
    latest: Option<LatestLedgerInfo>,
    velocity: Option<f64>,
    entries: &BTreeMap<String, RawLedgerEntry>,
) -> SentinelResult<()> {
    let Some(latest) = latest else {
        return Ok(());
    };
    SentinelSnapshot {
        header: SnapshotHeader {
            format_version: SNAPSHOT_FORMAT_VERSION,
            network_passphrase: network_passphrase.to_string(),
            protocol_version: latest.protocol_version,
        },
        data: SnapshotData {
            latest_ledger: latest,
            ledger_close_velocity_seconds: velocity,
            entries: entries.values().cloned().collect(),
        },
    }
    .write_deterministic(path)
}

#[async_trait]
impl LedgerReader for SnapshotLedgerReader {
    async fn get_latest_ledger(&self) -> SentinelResult<LatestLedgerInfo> {
        match &self.source {
            SnapshotLedgerSource::Replay(reader) => reader.get_latest_ledger().await,
            SnapshotLedgerSource::Recording {
                inner,
                path,
                network_passphrase,
                state,
                latest,
                velocity,
            } => {
                let latest_info = inner.get_latest_ledger().await?;
                *latest.write().await = Some(latest_info);
                persist_recording(
                    path,
                    network_passphrase,
                    Some(latest_info),
                    *velocity.read().await,
                    &*state.read().await,
                )
                .await?;
                Ok(latest_info)
            }
        }
    }

    async fn get_ledger_entries(
        &self,
        keys_xdr_base64: &[String],
    ) -> SentinelResult<Vec<RawLedgerEntry>> {
        match &self.source {
            SnapshotLedgerSource::Replay(reader) => {
                reader.get_ledger_entries(keys_xdr_base64).await
            }
            SnapshotLedgerSource::Recording {
                inner,
                path,
                network_passphrase,
                state,
                latest,
                velocity,
            } => {
                let fetched = inner.get_ledger_entries(keys_xdr_base64).await?;
                {
                    let mut entries = state.write().await;
                    for entry in &fetched {
                        entries.insert(entry.key_xdr_base64.clone(), entry.clone());
                    }
                }
                persist_recording(
                    path,
                    network_passphrase,
                    *latest.read().await,
                    *velocity.read().await,
                    &*state.read().await,
                )
                .await?;
                Ok(fetched)
            }
        }
    }

    async fn get_ledger_close_velocity(&self, latest_ledger: u32) -> SentinelResult<Option<f64>> {
        match &self.source {
            SnapshotLedgerSource::Replay(reader) => {
                reader.get_ledger_close_velocity(latest_ledger).await
            }
            SnapshotLedgerSource::Recording {
                inner,
                path,
                network_passphrase,
                state,
                latest,
                velocity,
            } => {
                let measured = inner.get_ledger_close_velocity(latest_ledger).await?;
                *velocity.write().await = measured;
                persist_recording(
                    path,
                    network_passphrase,
                    *latest.read().await,
                    measured,
                    &*state.read().await,
                )
                .await?;
                Ok(measured)
            }
        }
    }
}
