use super::raft_state::{RaftQueueCommand, RaftQueueOutcome, RaftQueueTypeConfig};
use super::raft_storage::QueueStorage;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use openraft::error::{
    ClientWriteError, InstallSnapshotError, NetworkError, RPCError, RaftError, RemoteError,
    Unreachable,
};
use openraft::network::{RPCOption, RaftNetwork, RaftNetworkFactory};
use openraft::raft::{
    AppendEntriesRequest, AppendEntriesResponse, InstallSnapshotRequest, InstallSnapshotResponse,
    VoteRequest, VoteResponse,
};
use openraft::{BasicNode, Config, Raft, SnapshotPolicy};
use sayangcare_core::domain::SessionId;
use sayangcare_core::ports::PriorityQueue;
use sayangcare_core::{CoreError, CoreResult};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

pub type QueueRaft = Raft<RaftQueueTypeConfig>;

pub const TOKEN_HEADER: &str = "x-raft-token";

#[derive(Debug, Clone)]
pub struct RaftQueueSettings {
    pub node_id: u64,
    /// Every cluster member (including this node) and its base URL, e.g. `http://api-1:8081`.
    /// Empty means a single-node cluster.
    pub peers: BTreeMap<u64, String>,
    /// File used to persist this node's Raft state; in-memory when `None`.
    pub data_file: Option<PathBuf>,
    /// Shared secret required on peer-to-peer requests when set.
    pub token: Option<String>,
}

impl RaftQueueSettings {
    /// Parses `1=http://host-a:8081,2=http://host-b:8081`.
    pub fn parse_peers(value: &str) -> Result<BTreeMap<u64, String>, String> {
        value
            .split(',')
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .map(|part| {
                let (id, url) = part
                    .split_once('=')
                    .ok_or_else(|| format!("invalid raft peer '{part}', expected id=url"))?;
                let id = id
                    .trim()
                    .parse::<u64>()
                    .map_err(|_| format!("invalid raft node id in '{part}'"))?;
                Ok((id, url.trim().trim_end_matches('/').to_string()))
            })
            .collect()
    }
}

/// Reply to a replicated write: the state-machine result and the log index that applied it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WriteReply {
    pub outcome: RaftQueueOutcome,
    pub log_index: u64,
}

pub struct RaftQueueNode {
    raft: QueueRaft,
    storage: Arc<QueueStorage>,
    http: reqwest::Client,
    settings: RaftQueueSettings,
}

impl RaftQueueNode {
    pub async fn start(settings: RaftQueueSettings) -> CoreResult<Arc<Self>> {
        if let Some(parent) = settings.data_file.as_ref().and_then(|file| file.parent()) {
            std::fs::create_dir_all(parent)
                .map_err(|e| CoreError::Storage(format!("raft data dir: {e}")))?;
        }
        let storage = QueueStorage::open(settings.data_file.clone())
            .map_err(|e| CoreError::Storage(format!("raft storage: {e}")))?;
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(3))
            .build()
            .map_err(|e| CoreError::Storage(format!("raft http client: {e}")))?;
        let config = Arc::new(
            Config {
                heartbeat_interval: 150,
                election_timeout_min: 600,
                election_timeout_max: 1200,
                snapshot_policy: SnapshotPolicy::LogsSinceLast(500),
                ..Default::default()
            }
            .validate()
            .map_err(|e| CoreError::Storage(format!("raft config: {e}")))?,
        );
        let (log_store, state_machine) = openraft::storage::Adaptor::new(storage.clone());
        let factory = HttpNetworkFactory {
            http: http.clone(),
            token: settings.token.clone(),
        };
        let raft = Raft::new(settings.node_id, config, factory, log_store, state_machine)
            .await
            .map_err(|e| CoreError::Storage(format!("raft start: {e}")))?;
        Ok(Arc::new(Self {
            raft,
            storage,
            http,
            settings,
        }))
    }

    pub fn raft(&self) -> &QueueRaft {
        &self.raft
    }

    pub fn token(&self) -> Option<&str> {
        self.settings.token.as_deref()
    }

    /// Every cluster member's base URL keyed by node id (empty for a single-node cluster).
    pub fn peers(&self) -> &BTreeMap<u64, String> {
        &self.settings.peers
    }

    /// Initializes a brand-new cluster. Only the lowest node id bootstraps; every other node
    /// waits to be added by its replication stream. Safe to call on every startup.
    pub async fn bootstrap_if_needed(&self) -> CoreResult<()> {
        let initialized = self
            .raft
            .is_initialized()
            .await
            .map_err(|e| CoreError::Storage(format!("raft init check: {e}")))?;
        if initialized {
            return Ok(());
        }
        let mut members: BTreeMap<u64, BasicNode> = self
            .settings
            .peers
            .iter()
            .map(|(id, url)| (*id, BasicNode::new(url.clone())))
            .collect();
        if members.is_empty() {
            members.insert(self.settings.node_id, BasicNode::new(""));
        }
        if members.keys().next() != Some(&self.settings.node_id) {
            return Ok(());
        }
        self.raft
            .initialize(members)
            .await
            .map_err(|e| CoreError::Storage(format!("raft initialize: {e}")))
    }

    /// Applies a command on this node only; fails when this node is not the leader.
    pub async fn write_local(&self, command: RaftQueueCommand) -> Result<WriteReply, String> {
        match self
            .raft
            .client_write::<tokio::sync::oneshot::error::RecvError>(command)
            .await
        {
            Ok(response) => Ok(WriteReply {
                outcome: response.data,
                log_index: response.log_id.index,
            }),
            Err(RaftError::APIError(ClientWriteError::ForwardToLeader(forward))) => Err(format!(
                "not leader; leader is {:?}",
                forward.leader_node.map(|node| node.addr)
            )),
            Err(error) => Err(error.to_string()),
        }
    }

    /// Replicates a command through the leader, forwarding when this node is a follower. Commands
    /// carry a request id, so retrying during elections cannot apply them twice.
    pub async fn write(&self, command: RaftQueueCommand) -> CoreResult<RaftQueueOutcome> {
        let mut last_error = String::from("no raft leader available");
        for attempt in 0..40 {
            if attempt > 0 {
                tokio::time::sleep(Duration::from_millis(150)).await;
            }
            let leader = match self
                .raft
                .client_write::<tokio::sync::oneshot::error::RecvError>(command.clone())
                .await
            {
                Ok(response) => {
                    self.await_applied(response.log_id.index).await;
                    return Ok(response.data);
                }
                Err(RaftError::APIError(ClientWriteError::ForwardToLeader(forward))) => {
                    forward.leader_node
                }
                Err(error) => {
                    last_error = error.to_string();
                    continue;
                }
            };
            let Some(leader) = leader else {
                continue;
            };
            match self.forward(&leader.addr, &command).await {
                Ok(reply) => {
                    self.await_applied(reply.log_index).await;
                    return Ok(reply.outcome);
                }
                Err(error) => last_error = error,
            }
        }
        Err(CoreError::Storage(format!(
            "raft write failed: {last_error}"
        )))
    }

    async fn forward(
        &self,
        leader: &str,
        command: &RaftQueueCommand,
    ) -> Result<WriteReply, String> {
        let mut request = self
            .http
            .post(format!("{leader}/internal/raft/write"))
            .json(command);
        if let Some(token) = &self.settings.token {
            request = request.header(TOKEN_HEADER, token);
        }
        let response = request.send().await.map_err(|e| e.to_string())?;
        response
            .json::<Result<WriteReply, String>>()
            .await
            .map_err(|e| e.to_string())?
    }

    /// Lets reads on this node observe the write it just made, even when it was forwarded.
    async fn await_applied(&self, index: u64) {
        let _ = self
            .raft
            .wait(Some(Duration::from_secs(2)))
            .applied_index_at_least(Some(index), "read-your-write")
            .await;
    }

    pub async fn leader_id(&self) -> Option<u64> {
        self.raft.current_leader().await
    }
}

#[derive(Clone)]
struct HttpNetworkFactory {
    http: reqwest::Client,
    token: Option<String>,
}

struct HttpNetwork {
    http: reqwest::Client,
    token: Option<String>,
    target: u64,
    base_url: String,
}

impl RaftNetworkFactory<RaftQueueTypeConfig> for HttpNetworkFactory {
    type Network = HttpNetwork;

    async fn new_client(&mut self, target: u64, node: &BasicNode) -> Self::Network {
        HttpNetwork {
            http: self.http.clone(),
            token: self.token.clone(),
            target,
            base_url: node.addr.trim_end_matches('/').to_string(),
        }
    }
}

impl HttpNetwork {
    async fn call<Req, Resp, E>(
        &self,
        path: &str,
        request: &Req,
    ) -> Result<Resp, RPCError<u64, BasicNode, RaftError<u64, E>>>
    where
        Req: Serialize,
        Resp: DeserializeOwned,
        E: std::error::Error + DeserializeOwned,
    {
        let mut builder = self
            .http
            .post(format!("{}/internal/raft/{path}", self.base_url))
            .json(request);
        if let Some(token) = &self.token {
            builder = builder.header(TOKEN_HEADER, token);
        }
        let response = builder
            .send()
            .await
            .map_err(|e| RPCError::Unreachable(Unreachable::new(&e)))?;
        let envelope = response
            .json::<Result<Resp, RaftError<u64, E>>>()
            .await
            .map_err(|e| RPCError::Network(NetworkError::new(&e)))?;
        envelope.map_err(|e| RPCError::RemoteError(RemoteError::new(self.target, e)))
    }
}

impl RaftNetwork<RaftQueueTypeConfig> for HttpNetwork {
    async fn append_entries(
        &mut self,
        rpc: AppendEntriesRequest<RaftQueueTypeConfig>,
        _option: RPCOption,
    ) -> Result<AppendEntriesResponse<u64>, RPCError<u64, BasicNode, RaftError<u64>>> {
        self.call("append", &rpc).await
    }

    async fn install_snapshot(
        &mut self,
        rpc: InstallSnapshotRequest<RaftQueueTypeConfig>,
        _option: RPCOption,
    ) -> Result<
        InstallSnapshotResponse<u64>,
        RPCError<u64, BasicNode, RaftError<u64, InstallSnapshotError>>,
    > {
        self.call("snapshot", &rpc).await
    }

    async fn vote(
        &mut self,
        rpc: VoteRequest<u64>,
        _option: RPCOption,
    ) -> Result<VoteResponse<u64>, RPCError<u64, BasicNode, RaftError<u64>>> {
        self.call("vote", &rpc).await
    }
}

/// Raft-replicated implementation of the volunteer priority queue.
///
/// Mutations go through the Raft log (leader-forwarded and idempotent); reads come from this
/// node's applied state machine.
pub struct RaftPriorityQueue {
    node: Arc<RaftQueueNode>,
}

impl RaftPriorityQueue {
    pub fn new(node: Arc<RaftQueueNode>) -> Self {
        Self { node }
    }

    pub fn node(&self) -> &Arc<RaftQueueNode> {
        &self.node
    }

    fn request_id() -> String {
        Uuid::new_v4().to_string()
    }

    async fn applied(&self, command: RaftQueueCommand) -> CoreResult<bool> {
        match self.node.write(command).await? {
            RaftQueueOutcome::Applied { ok } => Ok(ok),
            other => Err(CoreError::Storage(format!(
                "unexpected raft outcome {other:?}"
            ))),
        }
    }
}

#[async_trait]
impl PriorityQueue for RaftPriorityQueue {
    async fn enqueue(
        &self,
        session_id: SessionId,
        risk: u8,
        enqueued_at: DateTime<Utc>,
    ) -> CoreResult<()> {
        self.node
            .write(RaftQueueCommand::Enqueue {
                request_id: Self::request_id(),
                session_id,
                risk,
                enqueued_at,
            })
            .await
            .map(|_| ())
    }

    async fn list_pending(&self) -> CoreResult<Vec<SessionId>> {
        Ok(self.node.storage.inner.read().await.state.list_pending())
    }

    async fn peek_highest(&self) -> CoreResult<Option<SessionId>> {
        Ok(self.node.storage.inner.read().await.state.peek_highest())
    }

    async fn claim(&self, volunteer_id: &str) -> CoreResult<Option<SessionId>> {
        match self
            .node
            .write(RaftQueueCommand::ClaimHighest {
                request_id: Self::request_id(),
                volunteer_id: volunteer_id.to_string(),
            })
            .await?
        {
            RaftQueueOutcome::Claimed { session_id } => Ok(session_id),
            other => Err(CoreError::Storage(format!(
                "unexpected raft outcome {other:?}"
            ))),
        }
    }

    async fn claim_session(&self, session_id: &SessionId, volunteer_id: &str) -> CoreResult<bool> {
        self.applied(RaftQueueCommand::ClaimSession {
            request_id: Self::request_id(),
            session_id: session_id.clone(),
            volunteer_id: volunteer_id.to_string(),
        })
        .await
    }

    async fn list_claimed(&self, volunteer_id: &str) -> CoreResult<Vec<SessionId>> {
        Ok(self
            .node
            .storage
            .inner
            .read()
            .await
            .state
            .list_claimed(volunteer_id))
    }

    async fn move_claim(
        &self,
        session_id: &SessionId,
        from_volunteer_id: &str,
        to_volunteer_id: &str,
    ) -> CoreResult<bool> {
        self.applied(RaftQueueCommand::MoveClaim {
            request_id: Self::request_id(),
            session_id: session_id.clone(),
            from_volunteer_id: from_volunteer_id.to_string(),
            to_volunteer_id: to_volunteer_id.to_string(),
        })
        .await
    }

    async fn release_claim(&self, session_id: &SessionId, volunteer_id: &str) -> CoreResult<bool> {
        self.applied(RaftQueueCommand::ReleaseClaim {
            request_id: Self::request_id(),
            session_id: session_id.clone(),
            volunteer_id: volunteer_id.to_string(),
        })
        .await
    }
}
