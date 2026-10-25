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
use sayangcare_circuit_breaker::BreakerHandle;
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
    pub ai_slots: crate::performance::AiSlots,
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
    pub config: AppConfig,
}

impl AppState {
    pub async fn bootstrap(config: AppConfig) -> CoreResult<Self> {
        config
            .performance
            .validate()
            .map_err(|e| sayangcare_core::CoreError::Storage(e.to_string()))?;
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
            &config.circuit_breaker.key_prefix,
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

        Ok(Self {
            ai_slots: crate::performance::AiSlots::new(config.performance.max_ai_turns),
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
            config,
        })
    }

    /// Persist alerts in PostgreSQL so all API replicas share the same history.
    pub async fn record_alert(
        &self,
        kind: &str,
        session_id: &str,
        volunteer_id: Option<&str>,
        message: impl Into<String>,
    ) {
        if let Err(error) = self
            .try_record_alert(kind, session_id, volunteer_id, message)
            .await
        {
            tracing::error!(%error, session_id, "operator alert persistence failed");
        }
    }

    pub async fn try_record_alert(
        &self,
        kind: &str,
        session_id: &str,
        volunteer_id: Option<&str>,
        message: impl Into<String>,
    ) -> CoreResult<()> {
        let alert = OperatorAlert {
            // Initial escalation is one event per call; retries across replicas
            // must not produce duplicate dashboard alerts.
            id: if kind == "escalated" {
                format!("escalated:{session_id}")
            } else {
                Uuid::new_v4().to_string()
            },
            kind: kind.to_string(),
            session_id: session_id.to_string(),
            volunteer_id: volunteer_id.map(str::to_string),
            message: message.into(),
            created_at: Utc::now(),
        };
        sqlx::query(
            "INSERT INTO operator_alerts (id, kind, session_id, volunteer_id, message, created_at) SELECT $1, $2, $3, $4, $5, $6 WHERE $2 <> 'escalated' OR NOT EXISTS (SELECT 1 FROM operator_alerts WHERE session_id = $3 AND kind = 'escalated') ON CONFLICT (id) DO NOTHING"
        )
        .bind(&alert.id)
        .bind(&alert.kind)
        .bind(&alert.session_id)
        .bind(&alert.volunteer_id)
        .bind(&alert.message)
        .bind(alert.created_at)
        .execute(&self.postgres)
        .await.map_err(|error| sayangcare_core::CoreError::Storage(format!("operator alert persistence failed: {error}")))?;
        Ok(())
    }

    /// A saved escalation counter is also the retry intent for repeated-risk
    /// alerts. Stable per-event IDs make cancellation after commit safe to replay.
    pub async fn recover_risk_alerts(&self, session: &sayangcare_core::domain::Session) -> CoreResult<()> {
        self.try_record_alert("escalated", &session.id.0, None,
            format!("Session {} requires human support at risk level {}.", session.id, session.risk.level.0)).await?;
        sqlx::query("INSERT INTO operator_alerts (id, kind, session_id, message, created_at) SELECT 'repeated_risk:' || $1 || ':' || event::text, 'repeated_risk', $1, 'Repeated high-risk language requires operator review.', $3 FROM generate_series(2, $2::integer) AS event ON CONFLICT (id) DO NOTHING")
            .bind(&session.id.0).bind(i32::from(session.escalation_count)).bind(session.updated_at)
            .execute(&self.postgres).await
            .map_err(|e| sayangcare_core::CoreError::Storage(format!("repeated risk alert recovery: {e}")))?;
        Ok(())
    }

    pub async fn list_alerts(&self) -> Result<Vec<OperatorAlert>, sqlx::Error> {
        let rows: Vec<(String, String, String, Option<String>, String, DateTime<Utc>)> =
            sqlx::query_as(
                "SELECT id, kind, session_id, volunteer_id, message, created_at FROM operator_alerts ORDER BY created_at DESC, id DESC LIMIT 25"
            )
            .fetch_all(&self.postgres)
            .await?;
        Ok(rows
            .into_iter()
            .map(
                |(id, kind, session_id, volunteer_id, message, created_at)| OperatorAlert {
                    id,
                    kind,
                    session_id,
                    volunteer_id,
                    message,
                    created_at,
                },
            )
            .collect())
    }

    pub fn spawn_background_tasks(self: &Arc<Self>, cancel: CancellationToken) {
        Arc::new(OrphanedSessionSweeper::new(
            self.hot_sessions.clone(),
            self.tiered.clone(),
        ))
        .spawn(cancel.clone());

        // OBS-01: Periodically collect Redis session metrics.
        // How it works: every 30 seconds, the collector:
        // 1. Retrieves sessions using your existing scan_sessions() function.
        // 2. Classifies them into active sessions and archive backlog.
        // 3. Updates both Prometheus gauges.
        // 4. Logs a warning if the Redis scan fails, without incorrectly reporting zero sessions.
        let hot_sessions = self.hot_sessions.clone();
        let cancel_metrics = cancel.clone();
        let breakers = self.breakers.clone();

        tokio::spawn(async move {
            let mut tick = interval(Duration::from_secs(30));

            loop {
                tokio::select! {
                    _ = cancel_metrics.cancelled() => {
                        info!("operational metrics collector shutting down");
                        break;
                    }

                    _ = tick.tick() => {
                        match hot_sessions.scan_sessions().await {
                            Ok(sessions) => {
                                let (active, backlog) =
                                    crate::telemetry::classify_sessions(
                                        &sessions,
                                        Utc::now(),
                                    );

                                crate::telemetry::record_session_metrics(
                                    active,
                                    backlog,
                                );
                            }

                            Err(err) => {
                                tracing::warn!(
                                    error = %err,
                                    "failed to collect Redis session metrics"
                                );
                            }
                        }

                        for breaker_name in ["llm-sentiment", "llm-reply"] {
                            let breaker = breakers.get(breaker_name).await;
                            let state = breaker.current_state().await;

                            crate::telemetry::record_circuit_breaker_state(
                                breaker_name,
                                state,
                            );
                        }
                    }
                }
            }
        });

        let state = self.clone();
        let cancel_reaper = cancel.clone();
        tokio::spawn(async move {
            let mut tick = interval(Duration::from_secs(15));
            loop {
                tokio::select! {
                    _ = tick.tick() => {
                        if let Err(err) = state.reconcile_pending_handoffs().await {
                            tracing::warn!(error = %err, "pending handoff reconciliation failed");
                        }
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

    /// Saved session state is the retry intent, so API replacement does not
    /// lose a timed-out enqueue. Queue implementations preserve active claims.
    async fn reconcile_pending_handoffs(&self) -> CoreResult<()> {
        for candidate in self.hot_sessions.scan_sessions().await? {
            // Re-read immediately before replay: the scan may predate a claim
            // or a hangup. Redis/Raft enqueue also protects existing claims.
            let Some(session) = self.sessions.get(&candidate.id).await? else {
                continue;
            };
            if session.escalation_count == 0 {
                continue;
            }
            let alert = self.recover_risk_alerts(&session);
            let queue = async {
                if crate::handoff_recovery::needs_recovery(&session) {
                    self.queue.enqueue(session.id.clone(), session.risk.level.0.min(5), session.created_at).await
                } else {
                    Ok(())
                }
            };
            let (queued, alerted) = crate::handoff_recovery::replay(queue, alert).await;
            if let Err(error) = queued {
                tracing::warn!(%error, session_id = %session.id, "pending queue repair will retry");
            }
            if let Err(error) = alerted {
                tracing::warn!(%error, session_id = %session.id, "pending escalation alert repair will retry");
            }
        }
        // Calls can end before the next scan. Recover their initial alert from
        // the durable archive without putting ended calls back in the queue.
        sqlx::query("INSERT INTO operator_alerts (id, kind, session_id, message, created_at) SELECT 'escalated:' || s.id, 'escalated', s.id, 'Archived escalated call requires operator review.', s.updated_at FROM sessions s WHERE s.escalation_count > 0 AND NOT EXISTS (SELECT 1 FROM operator_alerts a WHERE a.session_id = s.id AND a.kind = 'escalated') LIMIT 100 ON CONFLICT (id) DO NOTHING")
            .execute(&self.postgres).await
            .map_err(|e| sayangcare_core::CoreError::Storage(format!("archived escalation alert recovery: {e}")))?;
        sqlx::query("INSERT INTO operator_alerts (id, kind, session_id, message, created_at) SELECT 'repeated_risk:' || s.id || ':' || event::text, 'repeated_risk', s.id, 'Repeated high-risk language requires operator review.', s.updated_at FROM sessions s CROSS JOIN LATERAL generate_series(2, s.escalation_count) AS event WHERE NOT EXISTS (SELECT 1 FROM operator_alerts a WHERE a.id = 'repeated_risk:' || s.id || ':' || event::text) LIMIT 100 ON CONFLICT (id) DO NOTHING")
            .execute(&self.postgres).await
            .map_err(|e| sayangcare_core::CoreError::Storage(format!("archived repeated risk alert recovery: {e}")))?;
        Ok(())
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
                        ).await;
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
