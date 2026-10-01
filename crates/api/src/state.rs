use sayangcare_circuit_breaker::{
    BreakerConfig, BreakerRegistry, SentimentAdaptivePolicy,
};
use sayangcare_core::config::AppConfig;
use sayangcare_core::ports::{ArchiveStore, PriorityQueue, SessionStore, TelephonyProvider};
use sayangcare_core::CoreResult;
use sayangcare_priority_queue::RedisPriorityQueue;
use sayangcare_session_store::{PostgresArchiveStore, RedisSessionStore, TieredSessionStore};
use sayangcare_telephony::TwilioProvider;
use std::sync::Arc;
use tokio::time::{interval, Duration};
use tracing::info;

use tokio_util::sync::CancellationToken;

pub struct AppState {
    pub sessions: Arc<dyn SessionStore>,
    pub archive: Arc<dyn ArchiveStore>,
    pub tiered: Arc<TieredSessionStore>,
    pub breakers: Arc<BreakerRegistry>,
    pub queue: Arc<dyn PriorityQueue>,
    pub telephony: Arc<dyn TelephonyProvider>,
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

        let tiered = Arc::new(TieredSessionStore::new(
            hot.clone(),
            archive.clone(),
        ));

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

        Ok(Self {
            sessions: tiered.clone(),
            archive,
            tiered,
            breakers,
            queue,
            telephony,
            config,
        })
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

    info!("background tasks started");
}

}