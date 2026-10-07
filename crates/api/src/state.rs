use chrono::{DateTime, Utc};
use sayangcare_circuit_breaker::{BreakerConfig, BreakerRegistry, SentimentAdaptivePolicy};
use sayangcare_core::config::AppConfig;
use sayangcare_core::domain::{HandoffStatus, SessionId};
use sayangcare_core::ports::{ArchiveStore, PriorityQueue, SessionStore, TelephonyProvider};
use sayangcare_core::CoreResult;
use uuid::Uuid;
use sayangcare_priority_queue::RedisPriorityQueue;
use sayangcare_session_store::{PostgresArchiveStore, RedisSessionStore, TieredSessionStore};
use sayangcare_telephony::TwilioProvider;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use tokio::time::{interval, Duration};
use tracing::info;

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
    pub sessions: Arc<dyn SessionStore>,
    pub archive: Arc<dyn ArchiveStore>,
    pub tiered: Arc<TieredSessionStore>,
    pub breakers: Arc<BreakerRegistry>,
    pub queue: Arc<dyn PriorityQueue>,
    pub telephony: Arc<dyn TelephonyProvider>,
    pub volunteers: Arc<RwLock<HashMap<String, Volunteer>>>,
    pub assigned_cases: Arc<RwLock<HashMap<String, Vec<String>>>>,
    pub alerts: Arc<RwLock<Vec<OperatorAlert>>>,
    pub config: AppConfig,
}

impl AppState {
    pub async fn bootstrap(config: AppConfig) -> CoreResult<Self> {
        // --- Hot store (Redis Sentinel) ---
        let hot = RedisSessionStore::new(
            config.redis.master_url.as_deref(),
            &config.redis.sentinel_endpoints,
            &config.redis.master_name,
            config.redis.password.as_deref(),
        )
        .await?;
        let hot = Arc::new(hot);

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

        let tiered = Arc::new(TieredSessionStore::new(hot.clone(), archive.clone()));

        // --- Circuit breaker ---
        let policy = Arc::new(SentimentAdaptivePolicy::new(
            config.circuit_breaker.base_error_rate,
            config.circuit_breaker.base_latency_ms,
            0.4, // sensitivity
        ));
        let breakers = Arc::new(BreakerRegistry::new(
            policy,
            BreakerConfig {
                base_error_rate: config.circuit_breaker.base_error_rate,
                base_latency_ms: config.circuit_breaker.base_latency_ms,
                min_requests: config.circuit_breaker.min_requests,
                half_open_after_secs: config.circuit_breaker.half_open_after_secs,
                window_size: 60,
            },
        ));

        // --- Priority queue (reuses Redis) ---
        let queue_url = config
            .redis
            .queue_url
            .as_deref()
            .or(config.redis.master_url.as_deref())
            .unwrap_or("redis://127.0.0.1:6379/");
        let redis_client = redis::Client::open(queue_url)
            .map_err(|e| sayangcare_core::CoreError::Storage(format!("redis: {e}")))?;
        let redis_mgr = redis::aio::ConnectionManager::new(redis_client)
            .await
            .map_err(|e| sayangcare_core::CoreError::Storage(format!("redis mgr: {e}")))?;
        let queue = Arc::new(RedisPriorityQueue::new(redis_mgr, "escalation:queue"));

        // --- Telephony ---
        let telephony = Arc::new(TwilioProvider::new(
            config.telephony.twilio_account_sid.clone(),
            config.telephony.twilio_auth_token.clone(),
        ));

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

        let assigned_cases = Arc::new(RwLock::new(HashMap::new()));
        let alerts = Arc::new(RwLock::new(Vec::new()));

        Ok(Self {
            sessions: tiered.clone(),
            archive,
            tiered,
            breakers,
            queue,
            telephony,
            volunteers,
            assigned_cases,
            alerts,
            config,
        })
    }

    pub fn record_alert(&self, kind: &str, session_id: &str, volunteer_id: Option<&str>, message: impl Into<String>) {
        let mut alerts = self
            .alerts
            .write()
            .expect("operator alert lock poisoned");
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
        // Circuit breaker ticker.
        let breakers = self.breakers.clone();
        let cancel_cb = cancel.clone();
        tokio::spawn(async move {
            let mut tick = interval(Duration::from_secs(5));
            loop {
                tokio::select! {
                    _ = tick.tick() => breakers.tick_all().await,
                    _ = cancel_cb.cancelled() => {
                        info!("breaker ticker shutting down");
                        break;
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

        info!("background tasks started");
    }

    async fn requeue_stale_handoffs(&self) -> CoreResult<()> {
        const HANDOFF_TIMEOUT: chrono::Duration = chrono::Duration::minutes(5);

        let stale_ids = {
            let mapping = self
                .assigned_cases
                .read()
                .expect("assigned cases lock poisoned");
            let mut stale = Vec::new();

            for (volunteer_id, session_ids) in mapping.iter() {
                for session_id in session_ids {
                    stale.push((volunteer_id.clone(), session_id.clone()));
                }
            }

            stale
        };

        for (volunteer_id, session_id) in stale_ids {
            let sid = SessionId(session_id.clone());
            match self.sessions.get(&sid).await {
                Ok(Some(session)) if session.is_handoff_stale(HANDOFF_TIMEOUT) => {
                    let expected = session.version;
                    let mut session = session;
                    session.assigned_volunteer_id = None;
                    session.handoff_status = HandoffStatus::Pending;
                    session.updated_at = Utc::now();
                    session.version += 1;

                    if let Err(err) = self.sessions.update_cas(&session, expected).await {
                        tracing::warn!(error = %err, session_id = %sid, volunteer_id = %volunteer_id, "failed to clear stale assignment");
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

                    self.record_alert(
                        "requeued",
                        &session_id,
                        Some(&volunteer_id),
                        format!("Stale assignment expired for session {} and was returned to the volunteer queue.", session_id),
                    );

                    let mut assigned_cases = self
                        .assigned_cases
                        .write()
                        .expect("assigned cases lock poisoned");
                    if let Some(cases) = assigned_cases.get_mut(&volunteer_id) {
                        cases.retain(|case_id| case_id != &session_id);
                        if cases.is_empty() {
                            assigned_cases.remove(&volunteer_id);
                        }
                    }

                    info!(session_id = %sid, volunteer_id = %volunteer_id, "stale handoff requeued for reassignment");
                }
                Ok(Some(_)) | Ok(None) => {}
                Err(err) => {
                    tracing::warn!(error = %err, session_id = %sid, volunteer_id = %volunteer_id, "stale handoff requeue aborted");
                }
            }
        }

        Ok(())
    }
}
