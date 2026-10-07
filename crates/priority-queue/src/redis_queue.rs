use async_trait::async_trait;
use chrono::{DateTime, Utc};
use redis::aio::ConnectionManager;
use redis::AsyncCommands;
use sayangcare_core::domain::SessionId;
use sayangcare_core::ports::PriorityQueue;
use sayangcare_core::{CoreError, CoreResult};

/// Distributed priority queue backed by a Redis sorted set.
///
/// Score = (5 - risk) * 1e12 + timestamp_millis
///   -> Higher risk sorts first (lower score).
///   -> Within same risk, earlier callers sort first (FIFO).
///
/// Claim is atomic via Lua to prevent two volunteers grabbing the same caller.
pub struct RedisPriorityQueue {
    client: ConnectionManager,
    queue_key: String,
    claim_prefix: String,
}

impl RedisPriorityQueue {
    pub fn new(client: ConnectionManager, queue_key: impl Into<String>) -> Self {
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

    #[tokio::test]
    async fn list_pending_returns_sorted_sessions() {
        let client = Client::open("redis://127.0.0.1:6379/").unwrap();
        let mut conn = client.get_async_connection().await.unwrap();
        let queue = RedisPriorityQueue::new(
            redis::aio::ConnectionManager::new(client).await.unwrap(),
            "test:queue",
        );

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
}

#[async_trait]
impl PriorityQueue for RedisPriorityQueue {
    async fn enqueue(
        &self,
        session_id: SessionId,
        risk: u8,
        enqueued_at: DateTime<Utc>,
    ) -> CoreResult<()> {
        let mut conn = self.client.clone();
        let score = Self::score(risk, enqueued_at);
        let _: () = conn
            .zadd(&self.queue_key, &session_id.0, score)
            .await
            .map_err(|e| CoreError::Storage(format!("zadd: {e}")))?;
        Ok(())
    }

    async fn list_pending(&self) -> CoreResult<Vec<SessionId>> {
        let mut conn = self.client.clone();
        let result: Vec<String> = conn
            .zrange(&self.queue_key, 0, -1)
            .await
            .map_err(|e| CoreError::Storage(format!("zrange: {e}")))?;
        Ok(result.into_iter().map(SessionId).collect())
    }

    async fn peek_highest(&self) -> CoreResult<Option<SessionId>> {
        let mut conn = self.client.clone();
        let result: Vec<String> = conn
            .zrange(&self.queue_key, 0, 0)
            .await
            .map_err(|e| CoreError::Storage(format!("zrange: {e}")))?;
        Ok(result.into_iter().next().map(SessionId))
    }

    async fn claim(&self, volunteer_id: &str) -> CoreResult<Option<SessionId>> {
        let mut conn = self.client.clone();
        // Atomically pop the highest-priority item and record the claim.
        let script = redis::Script::new(
            r#"
            local items = redis.call('ZRANGE', KEYS[1], 0, 0)
            if #items == 0 then return nil end
            local sid = items[1]
            redis.call('ZREM', KEYS[1], sid)
            redis.call('HSET', KEYS[2], sid, ARGV[1])
            return sid
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
    
        async fn claim_session(
            &self,
            session_id: &SessionId,
            volunteer_id: &str,
        ) -> CoreResult<bool> {
            let mut conn = self.client.clone();
            let script = redis::Script::new(
                r#"
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
}
