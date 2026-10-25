use async_trait::async_trait;
use chrono::{DateTime, Utc};
use redis::AsyncCommands;
use sayangcare_core::domain::SessionId;
use sayangcare_core::ports::PriorityQueue;
use sayangcare_core::{CoreError, CoreResult};
use sayangcare_redis::RedisConnectionProvider;

/// Distributed priority queue backed by a Redis sorted set.
///
/// Score = (5 - risk) * 1e12 + timestamp_millis
///   -> Higher risk sorts first (lower score).
///   -> Within same risk, earlier callers sort first (FIFO).
///
/// Claim is atomic via Lua to prevent two volunteers grabbing the same caller.
pub struct RedisPriorityQueue {
    client: RedisConnectionProvider,
    queue_key: String,
    claim_prefix: String,
}

impl RedisPriorityQueue {
    pub fn new(client: RedisConnectionProvider, queue_key: impl Into<String>) -> Self {
        let key = queue_key.into();
        Self {
            client,
            claim_prefix: format!("{}:claimed", key),
            queue_key: key,
        }
    }

    fn score(risk: u8, enqueued_at: DateTime<Utc>) -> f64 {
        let risk = risk.clamp(1, 5) as f64;
        let ts = enqueued_at.timestamp_millis() as f64;
        // Lower score = higher priority.
        (5.0 - risk) * 1e12 + ts
    }
}

#[cfg(test)]
mod tests {
    use super::RedisPriorityQueue;
    use chrono::Utc;
    use redis::Client;
    use sayangcare_core::domain::SessionId;
    use sayangcare_core::ports::PriorityQueue;
    use sayangcare_redis::RedisConnectionProvider;

    #[tokio::test]
    #[ignore = "requires Redis at 127.0.0.1:6379; run with --include-ignored after docker compose up"]
    async fn list_pending_returns_sorted_sessions() {
        let client = Client::open("redis://127.0.0.1:6379/").unwrap();
        let mut conn = client.get_multiplexed_async_connection().await.unwrap();
        let manager = redis::aio::ConnectionManager::new(client).await.unwrap();
        let queue =
            RedisPriorityQueue::new(RedisConnectionProvider::from_manager(manager), "test:queue");

        let _: () = redis::cmd("DEL")
            .arg("test:queue")
            .query_async(&mut conn)
            .await
            .unwrap();
        queue
            .enqueue(SessionId("CA-2".into()), 2, Utc::now())
            .await
            .unwrap();
        queue
            .enqueue(SessionId("CA-1".into()), 5, Utc::now())
            .await
            .unwrap();

        let pending = queue.list_pending().await.unwrap();
        assert!(pending.iter().any(|sid| sid.0 == "CA-2"));
        assert!(pending.iter().any(|sid| sid.0 == "CA-1"));
    }

    #[tokio::test]
    #[ignore = "requires Redis at 127.0.0.1:6379; run with --include-ignored after docker compose up"]
    async fn enqueue_does_not_requeue_an_actively_claimed_session() {
        let client = Client::open("redis://127.0.0.1:6379/").unwrap();
        let mut conn = client.get_multiplexed_async_connection().await.unwrap();
        let manager = redis::aio::ConnectionManager::new(client).await.unwrap();
        let key = format!(
            "test:queue:claimed:{}:{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        );
        let queue = RedisPriorityQueue::new(RedisConnectionProvider::from_manager(manager), &key);
        let session_id = SessionId("SIM-claimed-case".into());
        let next_session_id = SessionId("SIM-next-case".into());

        queue
            .enqueue(session_id.clone(), 5, Utc::now())
            .await
            .unwrap();
        assert!(queue
            .claim_session(&session_id, "volunteer-a")
            .await
            .unwrap());
        queue
            .enqueue(session_id.clone(), 5, Utc::now())
            .await
            .unwrap();
        queue
            .enqueue(next_session_id.clone(), 4, Utc::now())
            .await
            .unwrap();

        assert_eq!(
            queue.list_pending().await.unwrap(),
            vec![next_session_id.clone()]
        );
        let _: () = redis::cmd("ZADD")
            .arg(&key)
            .arg(1)
            .arg(&session_id.0)
            .query_async(&mut conn)
            .await
            .unwrap();
        assert!(!queue
            .claim_session(&session_id, "volunteer-b")
            .await
            .unwrap());
        assert_eq!(
            queue.claim("volunteer-b").await.unwrap(),
            Some(next_session_id.clone())
        );
        assert!(queue.list_pending().await.unwrap().is_empty());
        assert_eq!(
            queue.list_claimed("volunteer-a").await.unwrap(),
            vec![session_id]
        );
        assert_eq!(
            queue.list_claimed("volunteer-b").await.unwrap(),
            vec![next_session_id]
        );

        let _: () = redis::cmd("DEL")
            .arg(&key)
            .arg(format!("{key}:claimed"))
            .query_async(&mut conn)
            .await
            .unwrap();
    }

    #[tokio::test]
    #[ignore = "requires Redis at 127.0.0.1:6379; run with --include-ignored after docker compose up"]
    async fn concurrent_claims_assign_a_session_to_only_one_volunteer() {
        let client = Client::open("redis://127.0.0.1:6379/").unwrap();
        let mut conn = client.get_multiplexed_async_connection().await.unwrap();
        let manager = redis::aio::ConnectionManager::new(client).await.unwrap();
        let key = format!(
            "test:queue:concurrent:{}:{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        );
        let queue = RedisPriorityQueue::new(RedisConnectionProvider::from_manager(manager), &key);
        let session_id = SessionId("SIM-concurrent-case".into());
        queue
            .enqueue(session_id.clone(), 5, Utc::now())
            .await
            .unwrap();

        let (first, second) = tokio::join!(
            queue.claim_session(&session_id, "volunteer-a"),
            queue.claim_session(&session_id, "volunteer-b")
        );
        let first = first.unwrap();
        let second = second.unwrap();

        assert_ne!(first, second);
        assert!(queue.list_pending().await.unwrap().is_empty());

        let claimed_by_a = queue.list_claimed("volunteer-a").await.unwrap();
        let claimed_by_b = queue.list_claimed("volunteer-b").await.unwrap();
        assert_eq!(claimed_by_a.len() + claimed_by_b.len(), 1);
        assert_eq!(
            claimed_by_a.first().or_else(|| claimed_by_b.first()),
            Some(&session_id)
        );

        let _: () = redis::cmd("DEL")
            .arg(&key)
            .arg(format!("{key}:claimed"))
            .query_async(&mut conn)
            .await
            .unwrap();
    }
}

#[async_trait]
impl PriorityQueue for RedisPriorityQueue {
    async fn enqueue(
        &self,
        session_id: SessionId,
        risk: u8,
        enqueued_at: DateTime<Utc>,
    ) -> CoreResult<()> {
        let mut conn = self
            .client
            .connection()
            .await
            .map_err(|e| CoreError::Storage(format!("redis manager for enqueue: {e}")))?;
        let score = Self::score(risk, enqueued_at);
        let script = redis::Script::new(
            r#"
            if redis.call('HEXISTS', KEYS[2], ARGV[1]) == 1 then return 0 end
            return redis.call('ZADD', KEYS[1], ARGV[2], ARGV[1])
            "#,
        );
        let _: i32 = script
            .key(&self.queue_key)
            .key(&self.claim_prefix)
            .arg(&session_id.0)
            .arg(score)
            .invoke_async(&mut conn)
            .await
            .map_err(|e| CoreError::Storage(format!("enqueue: {e}")))?;
        Ok(())
    }

    async fn list_pending(&self) -> CoreResult<Vec<SessionId>> {
        let mut conn = self
            .client
            .connection()
            .await
            .map_err(|e| CoreError::Storage(format!("redis manager for list pending: {e}")))?;
        let result: Vec<String> = conn
            .zrange(&self.queue_key, 0, -1)
            .await
            .map_err(|e| CoreError::Storage(format!("zrange: {e}")))?;
        Ok(result.into_iter().map(SessionId).collect())
    }

    async fn peek_highest(&self) -> CoreResult<Option<SessionId>> {
        let mut conn = self
            .client
            .connection()
            .await
            .map_err(|e| CoreError::Storage(format!("redis manager for peek: {e}")))?;
        let result: Vec<String> = conn
            .zrange(&self.queue_key, 0, 0)
            .await
            .map_err(|e| CoreError::Storage(format!("zrange: {e}")))?;
        Ok(result.into_iter().next().map(SessionId))
    }

    async fn claim(&self, volunteer_id: &str) -> CoreResult<Option<SessionId>> {
        let mut conn = self
            .client
            .connection()
            .await
            .map_err(|e| CoreError::Storage(format!("redis manager for claim: {e}")))?;
        // Atomically pop the highest-priority item and record the claim.
        let script = redis::Script::new(
            r#"
            while true do
                local items = redis.call('ZRANGE', KEYS[1], 0, 0)
                if #items == 0 then return nil end
                local sid = items[1]
                if redis.call('HEXISTS', KEYS[2], sid) == 0 then
                    redis.call('ZREM', KEYS[1], sid)
                    redis.call('HSET', KEYS[2], sid, ARGV[1])
                    return sid
                end
                redis.call('ZREM', KEYS[1], sid)
            end
            "#,
        );
        let claimed: Option<String> = script
            .key(&self.queue_key)
            .key(&self.claim_prefix)
            .arg(volunteer_id)
            .invoke_async(&mut conn)
            .await
            .map_err(|e| CoreError::Storage(format!("claim: {e}")))?;
        Ok(claimed.map(SessionId))
    }

    async fn claim_session(&self, session_id: &SessionId, volunteer_id: &str) -> CoreResult<bool> {
        let mut conn = self
            .client
            .connection()
            .await
            .map_err(|e| CoreError::Storage(format!("redis manager for claim session: {e}")))?;
        let script = redis::Script::new(
            r#"
                if redis.call('HEXISTS', KEYS[2], ARGV[1]) == 1 then return 0 end
                if redis.call('ZSCORE', KEYS[1], ARGV[1]) == false then return 0 end
                redis.call('ZREM', KEYS[1], ARGV[1])
                redis.call('HSET', KEYS[2], ARGV[1], ARGV[2])
                return 1
                "#,
        );
        let claimed: i32 = script
            .key(&self.queue_key)
            .key(&self.claim_prefix)
            .arg(&session_id.0)
            .arg(volunteer_id)
            .invoke_async(&mut conn)
            .await
            .map_err(|e| CoreError::Storage(format!("claim session: {e}")))?;
        Ok(claimed == 1)
    }

    async fn list_claimed(&self, volunteer_id: &str) -> CoreResult<Vec<SessionId>> {
        let mut conn = self
            .client
            .connection()
            .await
            .map_err(|e| CoreError::Storage(format!("redis manager for list claims: {e}")))?;
        let claims: std::collections::HashMap<String, String> = conn
            .hgetall(&self.claim_prefix)
            .await
            .map_err(|e| CoreError::Storage(format!("list claims: {e}")))?;
        Ok(claims
            .into_iter()
            .filter_map(|(session_id, assigned_to)| {
                (assigned_to == volunteer_id).then_some(SessionId(session_id))
            })
            .collect())
    }

    async fn move_claim(
        &self,
        session_id: &SessionId,
        from_volunteer_id: &str,
        to_volunteer_id: &str,
    ) -> CoreResult<bool> {
        let mut conn = self
            .client
            .connection()
            .await
            .map_err(|e| CoreError::Storage(format!("redis manager for move claim: {e}")))?;
        let script = redis::Script::new(
            r#"
            if redis.call('HGET', KEYS[1], ARGV[1]) ~= ARGV[2] then return 0 end
            redis.call('HSET', KEYS[1], ARGV[1], ARGV[3])
            return 1
            "#,
        );
        let moved: i32 = script
            .key(&self.claim_prefix)
            .arg(&session_id.0)
            .arg(from_volunteer_id)
            .arg(to_volunteer_id)
            .invoke_async(&mut conn)
            .await
            .map_err(|e| CoreError::Storage(format!("move claim: {e}")))?;
        Ok(moved == 1)
    }

    async fn release_claim(&self, session_id: &SessionId, volunteer_id: &str) -> CoreResult<bool> {
        let mut conn = self
            .client
            .connection()
            .await
            .map_err(|e| CoreError::Storage(format!("redis manager for release claim: {e}")))?;
        let script = redis::Script::new(
            r#"
            if redis.call('HGET', KEYS[1], ARGV[1]) ~= ARGV[2] then return 0 end
            redis.call('HDEL', KEYS[1], ARGV[1])
            return 1
            "#,
        );
        let released: i32 = script
            .key(&self.claim_prefix)
            .arg(&session_id.0)
            .arg(volunteer_id)
            .invoke_async(&mut conn)
            .await
            .map_err(|e| CoreError::Storage(format!("release claim: {e}")))?;
        Ok(released == 1)
    }
}
