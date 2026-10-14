use crate::breaker::AdaptiveThresholdPolicy;
use crate::{BreakerConfig, CircuitBreaker, CircuitState, RequestPermit};
use async_trait::async_trait;
use sayangcare_core::{CoreError, CoreResult};
use sayangcare_redis::RedisConnectionProvider;
use std::sync::Arc;
use tracing::warn;
use uuid::Uuid;

const FAST_FAILURE_LIMIT: u64 = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedRequestPermit {
    generation: u64,
    probe_token: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BreakerPermit {
    Local(RequestPermit),
    Shared(SharedRequestPermit),
}

#[async_trait]
pub trait BreakerHandle: Send + Sync {
    async fn acquire_permit(&self) -> CoreResult<BreakerPermit>;
    async fn record_success(&self, permit: BreakerPermit, latency_ms: u64, distress_severity: f32);
    async fn record_failure(&self, permit: BreakerPermit, latency_ms: u64, distress_severity: f32);
    async fn current_state(&self) -> CircuitState;
}

#[async_trait]
impl BreakerHandle for CircuitBreaker {
    async fn acquire_permit(&self) -> CoreResult<BreakerPermit> {
        self.acquire_with_permit().map(BreakerPermit::Local)
    }

    async fn record_success(&self, permit: BreakerPermit, latency_ms: u64, distress_severity: f32) {
        if let BreakerPermit::Local(permit) = permit {
            self.record_success_with_distress(permit, latency_ms, distress_severity)
                .await;
        }
    }

    async fn record_failure(&self, permit: BreakerPermit, latency_ms: u64, distress_severity: f32) {
        if let BreakerPermit::Local(permit) = permit {
            self.record_failure_with_permit(permit, latency_ms, distress_severity)
                .await;
        }
    }

    async fn current_state(&self) -> CircuitState {
        self.state()
    }
}

pub struct SharedCircuitBreaker {
    name: String,
    prefix: String,
    client: RedisConnectionProvider,
    config: BreakerConfig,
    policy: Arc<dyn AdaptiveThresholdPolicy>,
    probe_lease_ms: u64,
}

impl SharedCircuitBreaker {
    pub fn new(
        name: impl Into<String>,
        prefix: impl Into<String>,
        client: RedisConnectionProvider,
        config: BreakerConfig,
        policy: Arc<dyn AdaptiveThresholdPolicy>,
        probe_lease_ms: u64,
    ) -> Self {
        Self {
            name: name.into(),
            prefix: prefix.into(),
            client,
            config,
            policy,
            probe_lease_ms,
        }
    }

    fn keys(&self) -> (String, String, String) {
        let base = format!("{}:{}", self.prefix, self.name);
        (
            format!("{base}:state"),
            format!("{base}:samples"),
            format!("{base}:sequence"),
        )
    }

    async fn record(
        &self,
        permit: SharedRequestPermit,
        failed: bool,
        latency_ms: u64,
        distress_severity: f32,
    ) {
        let (state_key, samples_key, sequence_key) = self.keys();
        let (error_threshold, latency_threshold) = self.policy.thresholds(distress_severity);
        let mut client = match self.client.connection().await {
            Ok(client) => client,
            Err(error) => {
                warn!(breaker = %self.name, %error, "shared circuit-breaker connection refresh failed");
                return;
            }
        };
        let script = redis::Script::new(RECORD_SCRIPT);
        let result: redis::RedisResult<i32> = script
            .key(state_key)
            .key(samples_key)
            .key(sequence_key)
            .arg(permit.generation)
            .arg(permit.probe_token.unwrap_or_default())
            .arg(i32::from(failed))
            .arg(latency_ms)
            .arg(error_threshold)
            .arg(latency_threshold)
            .arg(self.config.min_requests)
            .arg(self.config.window_size)
            .arg(self.config.half_open_after_secs.saturating_mul(1000))
            .arg(FAST_FAILURE_LIMIT)
            .invoke_async(&mut client)
            .await;
        if let Err(error) = result {
            warn!(breaker = %self.name, %error, "shared circuit-breaker record failed");
        }
    }
}

#[async_trait]
impl BreakerHandle for SharedCircuitBreaker {
    async fn acquire_permit(&self) -> CoreResult<BreakerPermit> {
        let candidate = Uuid::new_v4().to_string();
        let (state_key, _, _) = self.keys();
        let mut client = self.client.connection().await.map_err(|error| {
            warn!(breaker = %self.name, %error, "shared circuit-breaker connection refresh failed");
            CoreError::CircuitOpen(self.name.clone())
        })?;
        let script = redis::Script::new(ACQUIRE_SCRIPT);
        let result: redis::RedisResult<Vec<i64>> = script
            .key(state_key)
            .arg(&candidate)
            .arg(self.probe_lease_ms)
            .arg(self.config.half_open_after_secs.saturating_mul(1000))
            .invoke_async(&mut client)
            .await;
        let values = result.map_err(|error| {
            warn!(breaker = %self.name, %error, "shared circuit-breaker acquire failed closed");
            CoreError::CircuitOpen(self.name.clone())
        })?;
        let code = values.first().copied().unwrap_or(0);
        let generation = values.get(1).copied().unwrap_or(0).max(0) as u64;
        match code {
            1 => Ok(BreakerPermit::Shared(SharedRequestPermit {
                generation,
                probe_token: None,
            })),
            2 => Ok(BreakerPermit::Shared(SharedRequestPermit {
                generation,
                probe_token: Some(candidate),
            })),
            _ => Err(CoreError::CircuitOpen(self.name.clone())),
        }
    }

    async fn record_success(&self, permit: BreakerPermit, latency_ms: u64, distress_severity: f32) {
        if let BreakerPermit::Shared(permit) = permit {
            self.record(permit, false, latency_ms, distress_severity)
                .await;
        }
    }

    async fn record_failure(&self, permit: BreakerPermit, latency_ms: u64, distress_severity: f32) {
        if let BreakerPermit::Shared(permit) = permit {
            self.record(permit, true, latency_ms, distress_severity)
                .await;
        }
    }

    async fn current_state(&self) -> CircuitState {
        let (state_key, _, _) = self.keys();
        let mut client = match self.client.connection().await {
            Ok(client) => client,
            Err(error) => {
                warn!(breaker = %self.name, %error, "shared circuit-breaker connection refresh failed");
                return CircuitState::Open;
            }
        };
        let result: redis::RedisResult<i32> = redis::Script::new(STATE_SCRIPT)
            .key(state_key)
            .invoke_async(&mut client)
            .await;
        match result.unwrap_or(1) {
            0 => CircuitState::Closed,
            2 => CircuitState::HalfOpen,
            _ => CircuitState::Open,
        }
    }
}

const ACQUIRE_SCRIPT: &str = r#"
local time = redis.call('TIME')
local now = tonumber(time[1]) * 1000 + math.floor(tonumber(time[2]) / 1000)
local status = redis.call('HGET', KEYS[1], 'status') or 'CLOSED'
local generation = tonumber(redis.call('HGET', KEYS[1], 'generation') or '0')
if status == 'CLOSED' then return {1, generation} end
if status == 'OPEN' then
    local open_until = tonumber(redis.call('HGET', KEYS[1], 'open_until') or '0')
    if now < open_until then return {0, generation} end
    redis.call('HSET', KEYS[1], 'status', 'HALF_OPEN', 'probe_token', ARGV[1], 'probe_until', now + tonumber(ARGV[2]))
    return {2, generation}
end
local probe_until = tonumber(redis.call('HGET', KEYS[1], 'probe_until') or '0')
if probe_until <= now then
    redis.call('HSET', KEYS[1], 'status', 'OPEN', 'open_until', now + tonumber(ARGV[3]))
    redis.call('HINCRBY', KEYS[1], 'generation', 1)
end
return {0, tonumber(redis.call('HGET', KEYS[1], 'generation') or '0')}
"#;

const RECORD_SCRIPT: &str = r#"
local time = redis.call('TIME')
local now = tonumber(time[1]) * 1000 + math.floor(tonumber(time[2]) / 1000)
local status = redis.call('HGET', KEYS[1], 'status') or 'CLOSED'
local generation = tonumber(redis.call('HGET', KEYS[1], 'generation') or '0')
if generation ~= tonumber(ARGV[1]) then return 0 end
local token = ARGV[2]
local failed = tonumber(ARGV[3])
local latency = tonumber(ARGV[4])
local latency_limit = tonumber(ARGV[6])
local cooldown = tonumber(ARGV[9])
if token ~= '' then
    if status ~= 'HALF_OPEN' or redis.call('HGET', KEYS[1], 'probe_token') ~= token then return 0 end
    if failed == 1 or latency >= latency_limit then
        redis.call('HSET', KEYS[1], 'status', 'OPEN', 'open_until', now + cooldown)
        redis.call('HINCRBY', KEYS[1], 'generation', 1)
    else
        redis.call('HSET', KEYS[1], 'status', 'CLOSED')
        redis.call('HDEL', KEYS[1], 'open_until', 'probe_token', 'probe_until')
        redis.call('DEL', KEYS[2])
    end
    return 1
end
if status ~= 'CLOSED' then return 0 end
local sequence = redis.call('INCR', KEYS[3])
local member = string.format('%d:%020d:%d:%d', now, sequence, failed, latency)
redis.call('ZADD', KEYS[2], now, member)
local capacity = math.max(1, tonumber(ARGV[8]))
local count = redis.call('ZCARD', KEYS[2])
if count > capacity then redis.call('ZREMRANGEBYRANK', KEYS[2], 0, count - capacity - 1) end
local samples = redis.call('ZREVRANGE', KEYS[2], 0, -1)
local total = #samples
local failures = 0
local consecutive_failures = 0
local latencies = {}
for _, sample in ipairs(samples) do
    local sample_failed, sample_latency = string.match(sample, '^%d+:%d+:(%d+):(%d+)$')
    if sample_failed then
        local sample_failed_num = tonumber(sample_failed)
        table.insert(latencies, tonumber(sample_latency))
        failures = failures + sample_failed_num
    end
end
for _, sample in ipairs(samples) do
    local sample_failed = string.match(sample, '^%d+:%d+:(%d+):%d+$')
    if tonumber(sample_failed) == 1 then
        consecutive_failures = consecutive_failures + 1
    else
        break
    end
end
if consecutive_failures >= tonumber(ARGV[10]) then
    redis.call('HSET', KEYS[1], 'status', 'OPEN', 'open_until', now + cooldown)
    redis.call('HINCRBY', KEYS[1], 'generation', 1)
    return 1
end
local min_requests = tonumber(ARGV[7])
if total >= min_requests then
    table.sort(latencies)
    local p95 = latencies[math.ceil(total * 0.95)] or 0
    local error_rate = failures / total
    if error_rate >= tonumber(ARGV[5]) or p95 >= latency_limit then
        redis.call('HSET', KEYS[1], 'status', 'OPEN', 'open_until', now + cooldown)
        redis.call('HINCRBY', KEYS[1], 'generation', 1)
    end
end
return 1
"#;

const STATE_SCRIPT: &str = r#"
local time = redis.call('TIME')
local now = tonumber(time[1]) * 1000 + math.floor(tonumber(time[2]) / 1000)
local status = redis.call('HGET', KEYS[1], 'status') or 'CLOSED'
if status == 'OPEN' then
    local open_until = tonumber(redis.call('HGET', KEYS[1], 'open_until') or '0')
    if open_until <= now then return 2 end
    return 1
end
if status == 'HALF_OPEN' then return 2 end
return 0
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SentimentAdaptivePolicy;
    use redis::aio::ConnectionManager;

    #[tokio::test]
    #[ignore = "requires a local Redis service"]
    async fn redis_state_is_shared_and_only_one_replica_gets_the_probe() {
        let client = redis::Client::open("redis://127.0.0.1:6379/").unwrap();
        let manager = ConnectionManager::new(client).await.unwrap();
        let prefix = format!("test:sayangcare:breaker:{}", Uuid::new_v4());
        let config = BreakerConfig {
            base_error_rate: 0.5,
            base_latency_ms: 10_000,
            min_requests: 20,
            half_open_after_secs: 10,
            window_size: 60,
        };
        let policy = Arc::new(SentimentAdaptivePolicy::new(0.5, 10_000, 0.4));
        let provider = RedisConnectionProvider::from_manager(manager.clone());
        let first = SharedCircuitBreaker::new(
            "reply",
            &prefix,
            provider.clone(),
            config.clone(),
            policy.clone(),
            30_000,
        );
        let second = SharedCircuitBreaker::new("reply", &prefix, provider, config, policy, 30_000);

        for _ in 0..FAST_FAILURE_LIMIT {
            let permit = first.acquire_permit().await.unwrap();
            first.record_failure(permit, 10, 0.0).await;
        }
        assert_eq!(first.current_state().await, CircuitState::Open);
        assert_eq!(second.current_state().await, CircuitState::Open);
        assert!(second.acquire_permit().await.is_err());

        let (state_key, samples_key, sequence_key) = first.keys();
        let mut connection = manager.clone();
        let _: i32 = redis::cmd("HSET")
            .arg(&state_key)
            .arg("open_until")
            .arg(0)
            .query_async(&mut connection)
            .await
            .unwrap();
        let probe = first.acquire_permit().await.unwrap();
        assert!(second.acquire_permit().await.is_err());
        first.record_success(probe, 10, 0.0).await;
        assert_eq!(second.current_state().await, CircuitState::Closed);
        assert!(second.acquire_permit().await.is_ok());

        let mut connection = manager;
        let _: i32 = redis::cmd("DEL")
            .arg(state_key)
            .arg(samples_key)
            .arg(sequence_key)
            .query_async(&mut connection)
            .await
            .unwrap();
    }
}
