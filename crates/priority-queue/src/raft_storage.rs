use super::raft_state::{RaftQueueOutcome, RaftQueueState, RaftQueueTypeConfig};
use openraft::storage::{RaftLogReader, RaftSnapshotBuilder, RaftStorage};
use openraft::{
    BasicNode, Entry, EntryPayload, LogId, LogState, OptionalSend, Snapshot, SnapshotMeta,
    StorageError, StorageIOError, StoredMembership, Vote,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt::Debug;
use std::io::Cursor;
use std::ops::RangeBounds;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Clone, Serialize, Deserialize)]
struct StoredSnapshot {
    meta: SnapshotMeta<u64, BasicNode>,
    data: Vec<u8>,
}

#[derive(Default, Serialize, Deserialize)]
pub(crate) struct Persisted {
    vote: Option<Vote<u64>>,
    log: BTreeMap<u64, Entry<RaftQueueTypeConfig>>,
    last_purged: Option<LogId<u64>>,
    applied: Option<LogId<u64>>,
    membership: StoredMembership<u64, BasicNode>,
    pub(crate) state: RaftQueueState,
    snapshot: Option<StoredSnapshot>,
}

/// Raft log, vote, membership, state machine, and snapshot for one queue node.
///
/// With a file path the whole (small, snapshot-compacted) state is rewritten atomically after each
/// mutation so a restarted node keeps its vote, log, and queue contents; without one it is
/// in-memory only.
pub struct QueueStorage {
    pub(crate) inner: RwLock<Persisted>,
    path: Option<PathBuf>,
}

impl QueueStorage {
    pub fn open(path: Option<PathBuf>) -> std::io::Result<Arc<Self>> {
        let persisted = match &path {
            Some(path) if path.exists() => {
                serde_json::from_slice(&std::fs::read(path)?).map_err(std::io::Error::other)?
            }
            _ => Persisted::default(),
        };
        Ok(Arc::new(Self {
            inner: RwLock::new(persisted),
            path,
        }))
    }

    async fn persist(&self, data: &Persisted) -> Result<(), StorageError<u64>> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let bytes = serde_json::to_vec(data).map_err(io_error)?;
        let tmp = path.with_extension("tmp");
        tokio::fs::write(&tmp, bytes).await.map_err(io_error)?;
        tokio::fs::rename(&tmp, path).await.map_err(io_error)?;
        Ok(())
    }
}

fn io_error<E: std::error::Error + 'static>(error: E) -> StorageError<u64> {
    StorageIOError::write(&error).into()
}

impl RaftLogReader<RaftQueueTypeConfig> for Arc<QueueStorage> {
    async fn try_get_log_entries<RB: RangeBounds<u64> + Clone + Debug + OptionalSend>(
        &mut self,
        range: RB,
    ) -> Result<Vec<Entry<RaftQueueTypeConfig>>, StorageError<u64>> {
        let inner = self.inner.read().await;
        Ok(inner
            .log
            .range(range)
            .map(|(_, entry)| entry.clone())
            .collect())
    }
}

pub struct QueueSnapshotBuilder {
    storage: Arc<QueueStorage>,
}

impl RaftSnapshotBuilder<RaftQueueTypeConfig> for QueueSnapshotBuilder {
    async fn build_snapshot(&mut self) -> Result<Snapshot<RaftQueueTypeConfig>, StorageError<u64>> {
        let mut inner = self.storage.inner.write().await;
        let data = serde_json::to_vec(&inner.state).map_err(io_error)?;
        let meta = SnapshotMeta {
            last_log_id: inner.applied,
            last_membership: inner.membership.clone(),
            snapshot_id: format!("snapshot-{}", inner.applied.map_or(0, |id| id.index)),
        };
        inner.snapshot = Some(StoredSnapshot {
            meta: meta.clone(),
            data: data.clone(),
        });
        self.storage.persist(&inner).await?;
        Ok(Snapshot {
            meta,
            snapshot: Box::new(Cursor::new(data)),
        })
    }
}

impl RaftStorage<RaftQueueTypeConfig> for Arc<QueueStorage> {
    type LogReader = Self;
    type SnapshotBuilder = QueueSnapshotBuilder;

    async fn save_vote(&mut self, vote: &Vote<u64>) -> Result<(), StorageError<u64>> {
        let mut inner = self.inner.write().await;
        inner.vote = Some(*vote);
        self.persist(&inner).await
    }

    async fn read_vote(&mut self) -> Result<Option<Vote<u64>>, StorageError<u64>> {
        Ok(self.inner.read().await.vote)
    }

    async fn get_log_state(&mut self) -> Result<LogState<RaftQueueTypeConfig>, StorageError<u64>> {
        let inner = self.inner.read().await;
        let last_log_id = inner
            .log
            .values()
            .last()
            .map(|entry| entry.log_id)
            .or(inner.last_purged);
        Ok(LogState {
            last_purged_log_id: inner.last_purged,
            last_log_id,
        })
    }

    async fn get_log_reader(&mut self) -> Self::LogReader {
        self.clone()
    }

    async fn append_to_log<I>(&mut self, entries: I) -> Result<(), StorageError<u64>>
    where
        I: IntoIterator<Item = Entry<RaftQueueTypeConfig>> + OptionalSend,
    {
        let mut inner = self.inner.write().await;
        for entry in entries {
            inner.log.insert(entry.log_id.index, entry);
        }
        self.persist(&inner).await
    }

    async fn delete_conflict_logs_since(
        &mut self,
        log_id: LogId<u64>,
    ) -> Result<(), StorageError<u64>> {
        let mut inner = self.inner.write().await;
        inner.log.retain(|index, _| *index < log_id.index);
        self.persist(&inner).await
    }

    async fn purge_logs_upto(&mut self, log_id: LogId<u64>) -> Result<(), StorageError<u64>> {
        let mut inner = self.inner.write().await;
        inner.log.retain(|index, _| *index > log_id.index);
        inner.last_purged = Some(log_id);
        self.persist(&inner).await
    }

    async fn last_applied_state(
        &mut self,
    ) -> Result<(Option<LogId<u64>>, StoredMembership<u64, BasicNode>), StorageError<u64>> {
        let inner = self.inner.read().await;
        Ok((inner.applied, inner.membership.clone()))
    }

    async fn apply_to_state_machine(
        &mut self,
        entries: &[Entry<RaftQueueTypeConfig>],
    ) -> Result<Vec<RaftQueueOutcome>, StorageError<u64>> {
        let mut inner = self.inner.write().await;
        let mut results = Vec::with_capacity(entries.len());
        for entry in entries {
            let result = match &entry.payload {
                EntryPayload::Blank => RaftQueueOutcome::Claimed { session_id: None },
                EntryPayload::Normal(command) => inner.state.apply(command.clone()),
                EntryPayload::Membership(membership) => {
                    inner.membership =
                        StoredMembership::new(Some(entry.log_id), membership.clone());
                    RaftQueueOutcome::Claimed { session_id: None }
                }
            };
            inner.applied = Some(entry.log_id);
            results.push(result);
        }
        self.persist(&inner).await?;
        Ok(results)
    }

    async fn get_snapshot_builder(&mut self) -> Self::SnapshotBuilder {
        QueueSnapshotBuilder {
            storage: self.clone(),
        }
    }

    async fn begin_receiving_snapshot(
        &mut self,
    ) -> Result<Box<Cursor<Vec<u8>>>, StorageError<u64>> {
        Ok(Box::new(Cursor::new(Vec::new())))
    }

    async fn install_snapshot(
        &mut self,
        meta: &SnapshotMeta<u64, BasicNode>,
        snapshot: Box<Cursor<Vec<u8>>>,
    ) -> Result<(), StorageError<u64>> {
        let data = snapshot.into_inner();
        let state: RaftQueueState = serde_json::from_slice(&data).map_err(io_error)?;
        let mut inner = self.inner.write().await;
        inner.state = state;
        inner.applied = meta.last_log_id;
        inner.membership = meta.last_membership.clone();
        inner.snapshot = Some(StoredSnapshot {
            meta: meta.clone(),
            data,
        });
        self.persist(&inner).await
    }

    async fn get_current_snapshot(
        &mut self,
    ) -> Result<Option<Snapshot<RaftQueueTypeConfig>>, StorageError<u64>> {
        let inner = self.inner.read().await;
        Ok(inner.snapshot.as_ref().map(|stored| Snapshot {
            meta: stored.meta.clone(),
            snapshot: Box::new(Cursor::new(stored.data.clone())),
        }))
    }
}
