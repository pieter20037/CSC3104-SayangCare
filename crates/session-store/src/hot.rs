use async_trait::async_trait;
use redis::aio::{ConnectionManager, ConnectionManagerConfig};
use redis::sentinel::{Sentinel, SentinelNodeConnectionInfo};
use redis::AsyncCommands;
use redis::RedisConnectionInfo;
use sayangcare_core::domain::{Session, SessionId, SessionState};
use sayangcare_core::ports::SessionStore;
use sayangcare_core::{CoreError, CoreResult};
use std::time::Duration;

pub struct RedisSessionStore {
    client: ConnectionManager,
    ttl_secs: u64,
}

impl RedisSessionStore {
    pub async fn new(
        master_url: Option<&str>,
        sentinel_endpoints: &[String],
        master_name: &str,
        password: Option<&str>,
    ) -> CoreResult<Self> {
        let client = if let Some(master_url) = master_url {
            redis::Client::open(master_url)
                .map_err(|e| CoreError::Storage(format!("redis direct client: {e}")))?
        } else {
            let sentinel_urls: Vec<String> = sentinel_endpoints
                .iter()
                .map(|endpoint| format!("redis://{endpoint}/"))
                .collect();

            let node_info = SentinelNodeConnectionInfo {
                redis_connection_info: password.map(|password| RedisConnectionInfo {
                    password: Some(password.to_string()),
                    ..Default::default()
                }),
                ..Default::default()
            };
            let mut sentinel = Sentinel::build(sentinel_urls)
                .map_err(|e| CoreError::Storage(format!("sentinel init: {e}")))?;

            sentinel
                .async_master_for(master_name, Some(&node_info))
                .await
                .map_err(|e| CoreError::Storage(format!("sentinel master: {e}")))?
        };

        let mgr = ConnectionManager::new_with_config(
            client,
            ConnectionManagerConfig::new()
                .set_connection_timeout(Duration::from_secs(2))
                .set_response_timeout(Duration::from_millis(500)),
        )
        .await
        .map_err(|e| CoreError::Storage(format!("redis manager: {e}")))?;

        Ok(Self {
            client: mgr,
            ttl_secs: 3600, // 1h TTL on hot state
        })
    }

    fn is_terminal(state: &SessionState) -> bool {
        matches!(
            state,
            SessionState::Completed
                | SessionState::Failed
                | SessionState::Escalated
                | SessionState::Abandoned
        )
    }

    fn key(id: &SessionId) -> String {
        format!("session:{}", id.0)
    }

    /// Atomically delete only the version that was successfully archived.
    pub async fn delete_if_version(
        &self,
        id: &SessionId,
        expected_version: u64,
    ) -> CoreResult<bool> {
        let mut conn = self.client.clone();
        let script = redis::Script::new(
            r#"
            local current = redis.call('GET', KEYS[1])
            if not current then return 0 end
            local decoded = cjson.decode(current)
            if tonumber(decoded.version) ~= tonumber(ARGV[1]) then return 0 end
            redis.call('DEL', KEYS[1])
            return 1
            "#,
        );
        let deleted: i32 = script
            .key(Self::key(id))
            .arg(expected_version)
            .invoke_async(&mut conn)
            .await
            .map_err(|e| CoreError::Storage(format!("redis version-safe delete: {e}")))?;
        Ok(deleted == 1)
    }

    /// Scan the hot session namespace. SCAN avoids blocking Redis as KEYS would.
    pub async fn scan_sessions(&self) -> CoreResult<Vec<Session>> {
        let mut conn = self.client.clone();
        let mut cursor = 0_u64;
        let mut sessions = Vec::new();
        loop {
            let (next, keys): (u64, Vec<String>) = redis::cmd("SCAN")
                .arg(cursor)
                .arg("MATCH")
                .arg("session:*")
                .arg("COUNT")
                .arg(100)
                .query_async(&mut conn)
                .await
                .map_err(|e| CoreError::Storage(format!("redis scan sessions: {e}")))?;

            for key in keys {
                let raw: Option<String> = conn
                    .get(&key)
                    .await
                    .map_err(|e| CoreError::Storage(format!("redis read scanned session: {e}")))?;
                if let Some(raw) = raw {
                    let session = serde_json::from_str(&raw)
                        .map_err(|e| CoreError::Storage(format!("decode scanned session: {e}")))?;
                    sessions.push(session);
                }
            }

            cursor = next;
            if cursor == 0 {
                break;
            }
        }
        Ok(sessions)
    }
}

#[async_trait]
impl SessionStore for RedisSessionStore {
    async fn get(&self, id: &SessionId) -> CoreResult<Option<Session>> {
        let mut conn = self.client.clone();
        let raw: Option<String> = conn
            .get(Self::key(id))
            .await
            .map_err(|e| CoreError::Storage(format!("redis get: {e}")))?;

        match raw {
            Some(json) => Ok(Some(
                serde_json::from_str(&json)
                    .map_err(|e| CoreError::Storage(format!("decode: {e}")))?,
            )),
            None => Ok(None),
        }
    }

    async fn create_if_absent(&self, session: &Session) -> CoreResult<bool> {
        let mut conn = self.client.clone();
        let json = serde_json::to_string(session)
            .map_err(|e| CoreError::Storage(format!("encode: {e}")))?;
        // Redis runs this O(1)-key script atomically across pods; JSON encoding is O(session size).
        let script = redis::Script::new(
            r#"
            if redis.call('EXISTS', KEYS[1]) == 1 then return 0 end
            if ARGV[3] == '1' then
                redis.call('SET', KEYS[1], ARGV[2])
            else
                redis.call('SETEX', KEYS[1], ARGV[1], ARGV[2])
            end
            return 1
            "#,
        );
        let created: i32 = script
            .key(Self::key(&session.id))
            .arg(self.ttl_secs)
            .arg(json)
            .arg(if Self::is_terminal(&session.state) {
                1
            } else {
                0
            })
            .invoke_async(&mut conn)
            .await
            .map_err(|e| CoreError::Storage(format!("redis create: {e}")))?;

        Ok(created == 1)
    }

    async fn put(&self, session: &Session) -> CoreResult<()> {
        let mut conn = self.client.clone();
        let json = serde_json::to_string(session)
            .map_err(|e| CoreError::Storage(format!("encode: {e}")))?;
        if Self::is_terminal(&session.state) {
            let _: () = conn
                .set(Self::key(&session.id), json)
                .await
                .map_err(|e| CoreError::Storage(format!("redis set terminal: {e}")))?;
        } else {
            let _: () = conn
                .set_ex(Self::key(&session.id), json, self.ttl_secs)
                .await
                .map_err(|e| CoreError::Storage(format!("redis set: {e}")))?;
        }
        Ok(())
    }

    async fn update_cas(&self, session: &Session, expected_version: u64) -> CoreResult<()> {
        let mut conn = self.client.clone();
        // Lua: only write if current version matches expected.
        let script = redis::Script::new(
            r#"
            local current = redis.call('GET', KEYS[1])
            if not current then return -1 end
            local decoded = cjson.decode(current)
            if tonumber(decoded.version) ~= tonumber(ARGV[1]) then return 0 end
            if ARGV[4] == '1' then
                redis.call('SET', KEYS[1], ARGV[3])
            else
                redis.call('SETEX', KEYS[1], ARGV[2], ARGV[3])
            end
            return 1
            "#,
        );
        let json = serde_json::to_string(session)
            .map_err(|e| CoreError::Storage(format!("encode: {e}")))?;

        let result: i32 = script
            .key(Self::key(&session.id))
            .arg(expected_version)
            .arg(self.ttl_secs)
            .arg(json)
            .arg(if Self::is_terminal(&session.state) {
                1
            } else {
                0
            })
            .invoke_async(&mut conn)
            .await
            .map_err(|e| CoreError::Storage(format!("redis cas: {e}")))?;

        match result {
            1 => Ok(()),
            0 => Err(CoreError::VersionConflict {
                session_id: session.id.0.clone(),
                expected_version,
            }),
            _ => Err(CoreError::SessionNotFound(session.id.0.clone())),
        }
    }

    async fn delete_if_version(&self, id: &SessionId, expected_version: u64) -> CoreResult<bool> {
        RedisSessionStore::delete_if_version(self, id, expected_version).await
    }

    async fn delete(&self, id: &SessionId) -> CoreResult<()> {
        let mut conn = self.client.clone();
        let _: () = conn
            .del(Self::key(id))
            .await
            .map_err(|e| CoreError::Storage(format!("redis del: {e}")))?;
        Ok(())
    }
}
