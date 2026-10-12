#![cfg(test)]

use super::raft_state::{RaftQueueCommand, RaftQueueOutcome, RaftQueueState, RaftQueueTypeConfig};
use openraft::error::{RPCError, RaftError};
use openraft::network::{RPCOption, RaftNetwork, RaftNetworkFactory};
use openraft::raft::{
    AppendEntriesRequest, AppendEntriesResponse, InstallSnapshotRequest, InstallSnapshotResponse,
    VoteRequest, VoteResponse,
};
use openraft::storage::{RaftLogReader, RaftSnapshotBuilder, RaftStorage};
use openraft::{
    BasicNode, Config, Entry, EntryPayload, LogId, LogState, OptionalSend, Raft, Snapshot,
    SnapshotMeta, StorageError, StorageIOError, StoredMembership, Vote,
};
use std::collections::{BTreeMap, HashMap};
use std::fmt::Debug;
use std::io::Cursor;
use std::ops::RangeBounds;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use tokio::time::{sleep, Duration};

#[derive(Clone, Default)]
struct QueueStorage {
    vote: Arc<RwLock<Option<Vote<u64>>>>,
    log: Arc<RwLock<BTreeMap<u64, Entry<RaftQueueTypeConfig>>>>,
    applied: Arc<RwLock<Option<LogId<u64>>>>,
    membership: Arc<RwLock<StoredMembership<u64, BasicNode>>>,
    state: Arc<RwLock<RaftQueueState>>,
}

fn storage_error<E: std::error::Error + 'static>(error: E) -> StorageError<u64> {
    StorageIOError::read_state_machine(&error).into()
}

impl RaftLogReader<RaftQueueTypeConfig> for Arc<QueueStorage> {
    async fn try_get_log_entries<RB: RangeBounds<u64> + Clone + Debug + OptionalSend>(
        &mut self,
        range: RB,
    ) -> Result<Vec<Entry<RaftQueueTypeConfig>>, StorageError<u64>> {
        let log = self.log.read().await;
        Ok(log.range(range).map(|(_, entry)| entry.clone()).collect())
    }
}

struct SnapshotBuilder;

impl RaftSnapshotBuilder<RaftQueueTypeConfig> for SnapshotBuilder {
    async fn build_snapshot(&mut self) -> Result<Snapshot<RaftQueueTypeConfig>, StorageError<u64>> {
        Err(storage_error(std::io::Error::new(
            std::io::ErrorKind::Other,
            "snapshots are not used in the in-memory test cluster",
        )))
    }
}

impl RaftStorage<RaftQueueTypeConfig> for Arc<QueueStorage> {
    type LogReader = Self;
    type SnapshotBuilder = SnapshotBuilder;

    async fn save_vote(&mut self, vote: &Vote<u64>) -> Result<(), StorageError<u64>> {
        *self.vote.write().await = Some(vote.clone());
        Ok(())
    }

    async fn read_vote(&mut self) -> Result<Option<Vote<u64>>, StorageError<u64>> {
        Ok(self.vote.read().await.clone())
    }

    async fn get_log_state(&mut self) -> Result<LogState<RaftQueueTypeConfig>, StorageError<u64>> {
        let log = self.log.read().await;
        Ok(LogState {
            last_purged_log_id: None,
            last_log_id: log.values().last().map(|entry| entry.log_id),
        })
    }

    async fn get_log_reader(&mut self) -> Self::LogReader {
        self.clone()
    }

    async fn append_to_log<I>(&mut self, entries: I) -> Result<(), StorageError<u64>>
    where
        I: IntoIterator<Item = Entry<RaftQueueTypeConfig>> + OptionalSend,
    {
        let mut log = self.log.write().await;
        for entry in entries {
            log.insert(entry.log_id.index, entry);
        }
        Ok(())
    }

    async fn delete_conflict_logs_since(
        &mut self,
        log_id: LogId<u64>,
    ) -> Result<(), StorageError<u64>> {
        self.log
            .write()
            .await
            .retain(|index, _| *index < log_id.index);
        Ok(())
    }

    async fn purge_logs_upto(&mut self, log_id: LogId<u64>) -> Result<(), StorageError<u64>> {
        self.log
            .write()
            .await
            .retain(|index, _| *index > log_id.index);
        Ok(())
    }

    async fn last_applied_state(
        &mut self,
    ) -> Result<(Option<LogId<u64>>, StoredMembership<u64, BasicNode>), StorageError<u64>> {
        Ok((
            self.applied.read().await.clone(),
            self.membership.read().await.clone(),
        ))
    }

    async fn apply_to_state_machine(
        &mut self,
        entries: &[Entry<RaftQueueTypeConfig>],
    ) -> Result<Vec<RaftQueueOutcome>, StorageError<u64>> {
        let mut results = Vec::with_capacity(entries.len());
        for entry in entries {
            let result = match &entry.payload {
                EntryPayload::Blank => RaftQueueOutcome::Claimed { session_id: None },
                EntryPayload::Normal(command) => self.state.write().await.apply(command.clone()),
                EntryPayload::Membership(membership) => {
                    *self.membership.write().await =
                        StoredMembership::new(Some(entry.log_id), membership.clone());
                    RaftQueueOutcome::Claimed { session_id: None }
                }
            };
            *self.applied.write().await = Some(entry.log_id);
            results.push(result);
        }
        Ok(results)
    }

    async fn get_snapshot_builder(&mut self) -> Self::SnapshotBuilder {
        SnapshotBuilder
    }

    async fn begin_receiving_snapshot(
        &mut self,
    ) -> Result<Box<Cursor<Vec<u8>>>, StorageError<u64>> {
        Ok(Box::new(Cursor::new(Vec::new())))
    }

    async fn install_snapshot(
        &mut self,
        _meta: &SnapshotMeta<u64, BasicNode>,
        _snapshot: Box<Cursor<Vec<u8>>>,
    ) -> Result<(), StorageError<u64>> {
        Ok(())
    }

    async fn get_current_snapshot(
        &mut self,
    ) -> Result<Option<Snapshot<RaftQueueTypeConfig>>, StorageError<u64>> {
        Ok(None)
    }
}

type QueueRaft = Raft<RaftQueueTypeConfig>;

#[derive(Clone, Default)]
struct NetworkRegistry {
    nodes: Arc<Mutex<HashMap<u64, QueueRaft>>>,
    active: Arc<Mutex<std::collections::HashSet<u64>>>,
}

impl NetworkRegistry {
    fn for_node(&self, source: u64) -> NodeNetworkFactory {
        NodeNetworkFactory {
            registry: self.clone(),
            source,
        }
    }
}

#[derive(Clone)]
struct NodeNetworkFactory {
    registry: NetworkRegistry,
    source: u64,
}

#[derive(Clone)]
struct InMemoryNetwork {
    registry: NetworkRegistry,
    source: u64,
    target: u64,
}

impl RaftNetworkFactory<RaftQueueTypeConfig> for NodeNetworkFactory {
    type Network = InMemoryNetwork;

    async fn new_client(&mut self, target: u64, _node: &BasicNode) -> Self::Network {
        InMemoryNetwork {
            registry: self.registry.clone(),
            source: self.source,
            target,
        }
    }
}

impl RaftNetwork<RaftQueueTypeConfig> for InMemoryNetwork {
    async fn append_entries(
        &mut self,
        rpc: AppendEntriesRequest<RaftQueueTypeConfig>,
        _option: RPCOption,
    ) -> Result<AppendEntriesResponse<u64>, RPCError<u64, BasicNode, RaftError<u64>>> {
        let active = self.registry.active.lock().await;
        if !active.contains(&self.source) || !active.contains(&self.target) {
            return Err(RPCError::Unreachable(openraft::error::Unreachable::new(
                &std::io::Error::new(std::io::ErrorKind::NotConnected, "node unavailable"),
            )));
        }
        drop(active);
        let target = self.registry.nodes.lock().await.get(&self.target).cloned();
        target
            .ok_or_else(|| {
                RPCError::Unreachable(openraft::error::Unreachable::new(&std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "target missing",
                )))
            })?
            .append_entries(rpc)
            .await
            .map_err(|error| {
                RPCError::RemoteError(openraft::error::RemoteError::new(self.target, error))
            })
    }

    async fn install_snapshot(
        &mut self,
        rpc: InstallSnapshotRequest<RaftQueueTypeConfig>,
        _option: RPCOption,
    ) -> Result<
        InstallSnapshotResponse<u64>,
        RPCError<u64, BasicNode, RaftError<u64, openraft::error::InstallSnapshotError>>,
    > {
        let active = self.registry.active.lock().await;
        if !active.contains(&self.source) || !active.contains(&self.target) {
            return Err(RPCError::Unreachable(openraft::error::Unreachable::new(
                &std::io::Error::new(std::io::ErrorKind::NotConnected, "node unavailable"),
            )));
        }
        drop(active);
        let target = self.registry.nodes.lock().await.get(&self.target).cloned();
        target
            .ok_or_else(|| {
                RPCError::Unreachable(openraft::error::Unreachable::new(&std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "target missing",
                )))
            })?
            .install_snapshot(rpc)
            .await
            .map_err(|error| {
                RPCError::RemoteError(openraft::error::RemoteError::new(self.target, error))
            })
    }

    async fn vote(
        &mut self,
        rpc: VoteRequest<u64>,
        _option: RPCOption,
    ) -> Result<VoteResponse<u64>, RPCError<u64, BasicNode, RaftError<u64>>> {
        let active = self.registry.active.lock().await;
        if !active.contains(&self.source) || !active.contains(&self.target) {
            return Err(RPCError::Unreachable(openraft::error::Unreachable::new(
                &std::io::Error::new(std::io::ErrorKind::NotConnected, "node unavailable"),
            )));
        }
        drop(active);
        let target = self.registry.nodes.lock().await.get(&self.target).cloned();
        target
            .ok_or_else(|| {
                RPCError::Unreachable(openraft::error::Unreachable::new(&std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "target missing",
                )))
            })?
            .vote(rpc)
            .await
            .map_err(|error| {
                RPCError::RemoteError(openraft::error::RemoteError::new(self.target, error))
            })
    }
}

async fn new_node(id: u64, registry: NetworkRegistry) -> (QueueRaft, Arc<QueueStorage>) {
    let storage = Arc::new(QueueStorage::default());
    let (log_store, state_machine) = openraft::storage::Adaptor::new(storage.clone());
    let config = Arc::new(
        Config {
            heartbeat_interval: 50,
            election_timeout_min: 150,
            election_timeout_max: 300,
            ..Default::default()
        }
        .validate()
        .unwrap(),
    );
    let raft = Raft::new(id, config, registry.for_node(id), log_store, state_machine)
        .await
        .unwrap();
    (raft, storage)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn three_node_queue_replicates_and_survives_leader_loss() {
    let registry = NetworkRegistry::default();
    registry.active.lock().await.extend([1, 2, 3]);
    let (node1, state1) = new_node(1, registry.clone()).await;
    let (node2, state2) = new_node(2, registry.clone()).await;
    let (node3, state3) = new_node(3, registry.clone()).await;

    registry.nodes.lock().await.extend([
        (1, node1.clone()),
        (2, node2.clone()),
        (3, node3.clone()),
    ]);
    let members = BTreeMap::from([
        (1, BasicNode::new("node-1")),
        (2, BasicNode::new("node-2")),
        (3, BasicNode::new("node-3")),
    ]);
    node1.initialize(members).await.unwrap();
    sleep(Duration::from_millis(700)).await;
    assert_eq!(node1.current_leader().await, Some(1));

    let enqueue = RaftQueueCommand::Enqueue {
        request_id: "enqueue-1".to_string(),
        session_id: sayangcare_core::domain::SessionId("SIM-raft-1".to_string()),
        risk: 5,
        enqueued_at: chrono::Utc::now(),
    };
    node1
        .client_write::<tokio::sync::oneshot::error::RecvError>(enqueue)
        .await
        .unwrap();
    sleep(Duration::from_millis(250)).await;
    assert_eq!(
        state1.state.read().await.peek_highest(),
        state2.state.read().await.peek_highest()
    );
    assert_eq!(
        state2.state.read().await.peek_highest(),
        state3.state.read().await.peek_highest()
    );

    registry.active.lock().await.remove(&1);
    let mut new_leader = None;
    for _ in 0..40 {
        new_leader = node2
            .current_leader()
            .await
            .or(node3.current_leader().await);
        if matches!(new_leader, Some(2) | Some(3)) {
            break;
        }
        sleep(Duration::from_millis(100)).await;
    }
    assert!(matches!(new_leader, Some(2) | Some(3)));

    let leader = if node2.current_leader().await == Some(2) {
        node2
    } else {
        node3
    };
    leader
        .client_write::<tokio::sync::oneshot::error::RecvError>(RaftQueueCommand::ClaimHighest {
            request_id: "claim-1".to_string(),
            volunteer_id: "volunteer-aisha".to_string(),
        })
        .await
        .unwrap();
    sleep(Duration::from_millis(250)).await;
    assert_eq!(state2.state.read().await.peek_highest(), None);
    assert_eq!(state3.state.read().await.peek_highest(), None);
}
