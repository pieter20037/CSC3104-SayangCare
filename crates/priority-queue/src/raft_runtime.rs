use super::raft_node::{QueueRaft, RaftPriorityQueue, RaftQueueNode, RaftQueueSettings};
use super::raft_state::{RaftQueueCommand, RaftQueueOutcome, RaftQueueTypeConfig};
use super::raft_storage::QueueStorage;
use openraft::error::{InstallSnapshotError, RPCError, RaftError, Unreachable};
use openraft::network::{RPCOption, RaftNetwork, RaftNetworkFactory};
use openraft::raft::{
    AppendEntriesRequest, AppendEntriesResponse, InstallSnapshotRequest, InstallSnapshotResponse,
    VoteRequest, VoteResponse,
};
use openraft::{BasicNode, Config, Raft};
use sayangcare_core::domain::SessionId;
use sayangcare_core::ports::PriorityQueue;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::time::{sleep, Duration};

#[derive(Clone, Default)]
struct NetworkRegistry {
    nodes: Arc<Mutex<HashMap<u64, QueueRaft>>>,
    active: Arc<Mutex<HashSet<u64>>>,
}

#[derive(Clone)]
struct InMemoryNetwork {
    registry: NetworkRegistry,
    source: u64,
    target: u64,
}

impl InMemoryNetwork {
    async fn target<E: std::error::Error>(
        &self,
    ) -> Result<QueueRaft, RPCError<u64, BasicNode, RaftError<u64, E>>> {
        let unreachable = |reason: &str| {
            RPCError::Unreachable(Unreachable::new(&std::io::Error::new(
                std::io::ErrorKind::NotConnected,
                reason.to_string(),
            )))
        };
        let active = self.registry.active.lock().await;
        if !active.contains(&self.source) || !active.contains(&self.target) {
            return Err(unreachable("node unavailable"));
        }
        drop(active);
        self.registry
            .nodes
            .lock()
            .await
            .get(&self.target)
            .cloned()
            .ok_or_else(|| unreachable("target missing"))
    }
}

struct Factory {
    registry: NetworkRegistry,
    source: u64,
}

impl RaftNetworkFactory<RaftQueueTypeConfig> for Factory {
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
        self.target()
            .await?
            .append_entries(rpc)
            .await
            .map_err(|e| RPCError::RemoteError(openraft::error::RemoteError::new(self.target, e)))
    }

    async fn install_snapshot(
        &mut self,
        rpc: InstallSnapshotRequest<RaftQueueTypeConfig>,
        _option: RPCOption,
    ) -> Result<
        InstallSnapshotResponse<u64>,
        RPCError<u64, BasicNode, RaftError<u64, InstallSnapshotError>>,
    > {
        self.target()
            .await?
            .install_snapshot(rpc)
            .await
            .map_err(|e| RPCError::RemoteError(openraft::error::RemoteError::new(self.target, e)))
    }

    async fn vote(
        &mut self,
        rpc: VoteRequest<u64>,
        _option: RPCOption,
    ) -> Result<VoteResponse<u64>, RPCError<u64, BasicNode, RaftError<u64>>> {
        self.target()
            .await?
            .vote(rpc)
            .await
            .map_err(|e| RPCError::RemoteError(openraft::error::RemoteError::new(self.target, e)))
    }
}

async fn new_node(id: u64, registry: NetworkRegistry) -> (QueueRaft, Arc<QueueStorage>) {
    let storage = QueueStorage::open(None).unwrap();
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
    let factory = Factory {
        registry,
        source: id,
    };
    let raft = Raft::new(id, config, factory, log_store, state_machine)
        .await
        .unwrap();
    (raft, storage)
}

async fn wait_for_leader(nodes: &[&QueueRaft], allowed: &[u64]) -> u64 {
    for _ in 0..60 {
        for node in nodes {
            if let Some(leader) = node.current_leader().await {
                if allowed.contains(&leader) {
                    return leader;
                }
            }
        }
        sleep(Duration::from_millis(100)).await;
    }
    panic!("no leader elected among {allowed:?}");
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
    assert_eq!(wait_for_leader(&[&node1], &[1]).await, 1);

    let enqueue = RaftQueueCommand::Enqueue {
        request_id: "enqueue-1".to_string(),
        session_id: SessionId("SIM-raft-1".to_string()),
        risk: 5,
        enqueued_at: chrono::Utc::now(),
    };
    node1
        .client_write::<tokio::sync::oneshot::error::RecvError>(enqueue)
        .await
        .unwrap();
    sleep(Duration::from_millis(250)).await;
    for state in [&state1, &state2, &state3] {
        assert_eq!(
            state.inner.read().await.state.peek_highest(),
            Some(SessionId("SIM-raft-1".to_string()))
        );
    }

    registry.active.lock().await.remove(&1);
    let new_leader = wait_for_leader(&[&node2, &node3], &[2, 3]).await;
    let leader = if new_leader == 2 { &node2 } else { &node3 };
    let claim = leader
        .client_write::<tokio::sync::oneshot::error::RecvError>(RaftQueueCommand::ClaimHighest {
            request_id: "claim-1".to_string(),
            volunteer_id: "volunteer-aisha".to_string(),
        })
        .await
        .unwrap();
    assert_eq!(
        claim.data,
        RaftQueueOutcome::Claimed {
            session_id: Some(SessionId("SIM-raft-1".to_string()))
        }
    );
    sleep(Duration::from_millis(250)).await;
    assert_eq!(state2.inner.read().await.state.peek_highest(), None);
    assert_eq!(state3.inner.read().await.state.peek_highest(), None);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn single_node_raft_queue_implements_full_priority_queue_contract() {
    let node = RaftQueueNode::start(RaftQueueSettings {
        node_id: 1,
        peers: BTreeMap::new(),
        data_file: None,
        token: None,
    })
    .await
    .unwrap();
    node.bootstrap_if_needed().await.unwrap();
    let queue = RaftPriorityQueue::new(node);
    let now = chrono::Utc::now();
    let id = |value: &str| SessionId(value.to_string());

    queue.enqueue(id("low"), 4, now).await.unwrap();
    queue.enqueue(id("high"), 5, now).await.unwrap();
    assert_eq!(
        queue.list_pending().await.unwrap(),
        vec![id("high"), id("low")]
    );
    assert_eq!(queue.peek_highest().await.unwrap(), Some(id("high")));

    assert!(queue.claim_session(&id("low"), "aisha").await.unwrap());
    assert!(!queue.claim_session(&id("low"), "noor").await.unwrap());
    assert_eq!(queue.claim("noor").await.unwrap(), Some(id("high")));
    assert_eq!(queue.claim("noor").await.unwrap(), None);
    assert_eq!(queue.list_claimed("aisha").await.unwrap(), vec![id("low")]);

    assert!(queue.move_claim(&id("low"), "aisha", "noor").await.unwrap());
    assert!(!queue.release_claim(&id("low"), "aisha").await.unwrap());
    assert!(queue.release_claim(&id("low"), "noor").await.unwrap());
    queue.enqueue(id("low"), 4, now).await.unwrap();
    assert_eq!(queue.peek_highest().await.unwrap(), Some(id("low")));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn raft_state_survives_node_restart() {
    let path = std::env::temp_dir().join(format!("raft-restart-{}.json", uuid::Uuid::new_v4()));
    let settings = RaftQueueSettings {
        node_id: 1,
        peers: BTreeMap::new(),
        data_file: Some(path.clone()),
        token: None,
    };
    {
        let node = RaftQueueNode::start(settings.clone()).await.unwrap();
        node.bootstrap_if_needed().await.unwrap();
        let queue = RaftPriorityQueue::new(node.clone());
        queue
            .enqueue(SessionId("durable".to_string()), 5, chrono::Utc::now())
            .await
            .unwrap();
        node.raft().shutdown().await.unwrap();
    }

    let node = RaftQueueNode::start(settings).await.unwrap();
    node.bootstrap_if_needed().await.unwrap();
    let queue = RaftPriorityQueue::new(node);
    assert_eq!(
        queue.peek_highest().await.unwrap(),
        Some(SessionId("durable".to_string()))
    );
    let _ = std::fs::remove_file(path);
}
