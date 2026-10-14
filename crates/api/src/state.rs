use crate::inference::GroqInference;
use chrono::{DateTime, Utc};
use sayangcare_circuit_breaker::{BreakerConfig, BreakerRegistry, SentimentAdaptivePolicy};
use sayangcare_core::config::AppConfig;
use sayangcare_core::domain::{HandoffStatus, SessionId};
use sayangcare_core::ports::{
    ArchiveStore, InferenceService, PriorityQueue, SessionStore, TelephonyProvider,
};
use sayangcare_core::CoreResult;
use sayangcare_priority_queue::{
    RaftPriorityQueue, RaftQueueNode, RaftQueueSettings, RedisPriorityQueue,
};
use sayangcare_redis::RedisConnectionProvider;
use sayangcare_session_store::{
    OrphanedSessionSweeper, PostgresArchiveStore, RedisSessionStore, TieredSessionStore,
    VolunteerBindingStore,
};
use sayangcare_telephony::TwilioProvider;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use tokio::time::{interval, Duration};
use tracing::info;
use uuid::Uuid;

use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Volunteer {
    pub id: String,
    pub display_name: String,
    pub phone_number: String,
    pub on_shift: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperatorAlert {
    pub id: String,
    pub kind: String,
    pub session_id: String,
    pub volunteer_id: Option<String>,
    pub message: String,
    pub created_at: DateTime<Utc>,
}

pub struct AppState {
    pub hot_sessions: Arc<RedisSessionStore>,
    pub postgres: sqlx::PgPool,
    pub sessions: Arc<dyn SessionStore>,
    pub archive: Arc<dyn ArchiveStore>,
    pub tiered: Arc<TieredSessionStore>,
    pub breakers: Arc<BreakerRegistry>,
    pub queue: Arc<dyn PriorityQueue>,
    pub raft: Option<Arc<RaftQueueNode>>,
    pub volunteer_bindings: Arc<VolunteerBindingStore>,
    pub telephony: Arc<dyn TelephonyProvider>,
    pub inference: Option<Arc<dyn InferenceService>>,
    pub volunteers: Arc<RwLock<HashMap<String, Volunteer>>>,
    pub alerts: Arc<RwLock<Vec<OperatorAlert>>>,
    pub config: AppConfig,
}

impl AppState {
    pub async fn bootstrap(config: AppConfig) -> CoreResult<Self> {
        // --- Hot store (Redis Sentinel) ---
        let redis = RedisConnectionProvider::from_config(
            config.redis.master_url.as_deref(),
            &config.redis.sentinel_endpoints,
            &config.redis.master_name,
            config.redis.password.as_deref(),
        )
        .await
        .map_err(|error| {
            sayangcare_core::CoreError::Storage(format!("redis connection provider: {error}"))
        })?;
        let hot = Arc::new(RedisSessionStore::new(redis.clone()));

        // --- Cold store (Postgres) ---
        let pg_pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(config.postgres.max_connections)
            .connect(&config.postgres.url)
            .await
            .map_err(|e| sayangcare_core::CoreError::Storage(format!("pg pool: {e}")))?;
        sqlx::migrate!("../../migrations")
            .run(&pg_pool)
            .await
            .map_err(|e| sayangcare_core::CoreError::Storage(format!("migrate: {e}")))?;
        let archive = Arc::new(PostgresArchiveStore::new(pg_pool.clone()));
        let volunteer_bindings = Arc::new(VolunteerBindingStore::new(pg_pool.clone()));

        let tiered = Arc::new(TieredSessionStore::new(hot.clone(), archive.clone()));

        // --- Circuit breaker ---
        let policy = Arc::new(SentimentAdaptivePolicy::new(
            config.circuit_breaker.base_error_rate,
            config.circuit_breaker.base_latency_ms,
            0.4, // sensitivity
        ));
        let probe_lease_ms = config
            .ai
            .timeout_secs
            .max(1)
            .saturating_mul(2)
            .saturating_add(5)
            .max(30)
            .saturating_mul(1000);
        let breakers = Arc::new(BreakerRegistry::new(
            policy,
            BreakerConfig {
                base_error_rate: config.circuit_breaker.base_error_rate,
                base_latency_ms: config.circuit_breaker.base_latency_ms,
                min_requests: config.circuit_breaker.min_requests,
                half_open_after_secs: config.circuit_breaker.half_open_after_secs,
                window_size: 60,
            },
            redis.clone(),
            "sayangcare:circuit-breaker",
            probe_lease_ms,
        ));

        // --- Priority queue: Redis sorted set (default) or Raft-replicated cluster ---
        let (queue, raft): (Arc<dyn PriorityQueue>, Option<Arc<RaftQueueNode>>) =
            match config.queue.backend.to_ascii_lowercase().as_str() {
                "raft" => {
                    let peers = RaftQueueSettings::parse_peers(&config.queue.raft_peers)
                        .map_err(sayangcare_core::CoreError::Storage)?;
                    let node_id = config.queue.raft_node_id;
                    if !peers.is_empty() && !peers.contains_key(&node_id) {
                        return Err(sayangcare_core::CoreError::Storage(format!(
                            "raft node id {node_id} is not listed in raft_peers"
                        )));
                    }
                    let node = RaftQueueNode::start(RaftQueueSettings {
                        node_id,
                        peers,
                        data_file: config.queue.raft_data_dir.as_ref().map(|dir| {
                            std::path::Path::new(dir).join(format!("raft-node-{node_id}.json"))
                        }),
                        token: config.queue.raft_token.clone().filter(|t| !t.is_empty()),
                    })
                    .await?;
                    info!(node_id, "Raft priority queue enabled");
                    (Arc::new(RaftPriorityQueue::new(node.clone())), Some(node))
                }
                "redis" => (
                    Arc::new(RedisPriorityQueue::new(redis.clone(), "escalation:queue")),
                    None,
                ),
                other => {
                    return Err(sayangcare_core::CoreError::Storage(format!(
                        "unknown queue backend '{other}'; use 'redis' or 'raft'"
                    )))
                }
            };

        // --- Telephony ---
        let telephony = Arc::new(TwilioProvider::new(
            config.telephony.twilio_account_sid.clone(),
            config.telephony.twilio_auth_token.clone(),
        ));

        let inference: Option<Arc<dyn InferenceService>> = GroqInference::new(&config.ai)?
            .map(|provider| Arc::new(provider) as Arc<dyn InferenceService>);
        if inference.is_some() {
            info!(model = %config.ai.model, "Groq inference provider configured");
        } else {
            info!("Groq inference provider not configured; deterministic replies will be used");
        }

        let volunteers = Arc::new(RwLock::new(HashMap::from([
            (
                "volunteer-aisha".to_string(),
                Volunteer {
                    id: "volunteer-aisha".to_string(),
                    display_name: "Aisha Rahman".to_string(),
                    phone_number: "+6590001001".to_string(),
                    on_shift: true,
                },
            ),
            (
                "volunteer-noor".to_string(),
                Volunteer {
                    id: "volunteer-noor".to_string(),
                    display_name: "Noor Hazim".to_string(),
                    phone_number: "+6590001002".to_string(),
                    on_shift: true,
                },
            ),
            (
                "volunteer-ibrahim".to_string(),
                Volunteer {
                    id: "volunteer-ibrahim".to_string(),
                    display_name: "Ibrahim Lim".to_string(),
                    phone_number: "+6590001003".to_string(),
                    on_shift: false,
                },
            ),
        ])));

        let alerts = Arc::new(RwLock::new(Vec::new()));

        Ok(Self {
            hot_sessions: hot,
            postgres: pg_pool,
            sessions: tiered.clone(),
            archive,
            tiered,
            breakers,
            queue,
            raft,
            volunteer_bindings,
            telephony,
            inference,
            volunteers,
            alerts,
            config,
        })
    }

    pub fn record_alert(
        &self,
        kind: &str,
        session_id: &str,
        volunteer_id: Option<&str>,
        message: impl Into<String>,
    ) {
        let mut alerts = self.alerts.write().expect("operator alert lock poisoned");
        let alert = OperatorAlert {
            id: Uuid::new_v4().to_string(),
            kind: kind.to_string(),
            session_id: session_id.to_string(),
            volunteer_id: volunteer_id.map(str::to_string),
            message: message.into(),
            created_at: Utc::now(),
        };
        alerts.insert(0, alert);
        alerts.truncate(25);
    }

    pub fn list_alerts(&self) -> Vec<OperatorAlert> {
        self.alerts
            .read()
            .expect("operator alert lock poisoned")
            .iter()
            .cloned()
            .collect()
    }

    pub fn spawn_background_tasks(self: &Arc<Self>, cancel: CancellationToken) {
        Arc::new(OrphanedSessionSweeper::new(
            self.hot_sessions.clone(),
            self.tiered.clone(),
        ))
        .spawn(cancel.clone());

        let state = self.clone();
        let cancel_reaper = cancel.clone();
        tokio::spawn(async move {
            let mut tick = interval(Duration::from_secs(15));
            loop {
                tokio::select! {
                    _ = tick.tick() => {
                        if let Err(err) = state.requeue_stale_handoffs().await {
                            tracing::warn!(error = %err, "stale handoff reaper failed");
                        }
                    }
                    _ = cancel_reaper.cancelled() => {
                        info!("handoff reaper shutting down");
                        break;
                    }
                }
            }
        });

        if let Some(node) = self.raft.clone() {
            let cancel_raft = cancel.clone();
            tokio::spawn(async move {
                for _ in 0..30 {
                    match node.bootstrap_if_needed().await {
                        Ok(()) => break,
                        Err(err) => tracing::warn!(error = %err, "raft bootstrap failed; retrying"),
                    }
                    tokio::select! {
                        _ = tokio::time::sleep(Duration::from_secs(2)) => {}
                        _ = cancel_raft.cancelled() => return,
                    }
                }
                cancel_raft.cancelled().await;
                let _ = node.raft().shutdown().await;
            });
        }

        info!("background tasks started");
    }

    async fn requeue_stale_handoffs(&self) -> CoreResult<()> {
        const HANDOFF_TIMEOUT: chrono::Duration = chrono::Duration::minutes(5);

        let stale_ids = self
            .volunteer_bindings
            .list_stale_before(Utc::now() - HANDOFF_TIMEOUT)
            .await?;

        for (volunteer_id, session_id) in stale_ids {
            let sid = SessionId(session_id.clone());
            match self.sessions.get(&sid).await {
                Ok(Some(mut session)) => {
                    if session.is_handoff_stale(HANDOFF_TIMEOUT) {
                        let expected = session.version;
                        session.assigned_volunteer_id = None;
                        session.handoff_status = HandoffStatus::Pending;
                        session.updated_at = Utc::now();
                        session.version += 1;

                        if let Err(err) = self.sessions.update_cas(&session, expected).await {
                            tracing::warn!(error = %err, session_id = %sid, volunteer_id = %volunteer_id, "failed to clear stale assignment");
                            continue;
                        }

                        self.record_alert(
                            "requeued",
                            &session_id,
                            Some(&volunteer_id),
                            format!("Stale assignment expired for session {} and was returned to the volunteer queue.", session_id),
                        );
                    } else if session.handoff_status != HandoffStatus::Pending
                        || session.assigned_volunteer_id.is_some()
                    {
                        continue;
                    }

                    if let Err(err) = self.queue.release_claim(&sid, &volunteer_id).await {
                        tracing::warn!(error = %err, session_id = %sid, volunteer_id = %volunteer_id, "failed to release stale Redis queue claim");
                        continue;
                    }
                    if let Err(err) = self
                        .queue
                        .enqueue(sid.clone(), session.risk.level.0, Utc::now())
                        .await
                    {
                        tracing::warn!(error = %err, session_id = %sid, volunteer_id = %volunteer_id, "failed to requeue stale session");
                        continue;
                    }
                    if let Err(err) = self
                        .volunteer_bindings
                        .release(&session_id, &volunteer_id)
                        .await
                    {
                        tracing::warn!(error = %err, session_id = %sid, volunteer_id = %volunteer_id, "failed to close stale durable binding");
                        continue;
                    }
                    info!(session_id = %sid, volunteer_id = %volunteer_id, "stale handoff requeued for reassignment");
                }
                Ok(None) => {
                    let _ = self.queue.release_claim(&sid, &volunteer_id).await;
                    self.volunteer_bindings
                        .release(&session_id, &volunteer_id)
                        .await?;
                }
                Err(err) => {
                    tracing::warn!(error = %err, session_id = %sid, volunteer_id = %volunteer_id, "stale handoff requeue aborted");
                }
            }
        }

        Ok(())
    }
}
