use async_trait::async_trait;
use sayangcare_core::domain::{Session, SessionId};
use sayangcare_core::ports::SessionStore;
use sayangcare_core::{CoreError, CoreResult};
use sqlx::PgPool;
use std::sync::Arc;
use tokio::time::{timeout, Duration};
use tracing::warn;

#[cfg(test)]
#[path = "checkpoint_tests.rs"]
mod tests;

/// PostgreSQL is the commit authority; Redis is a disposable hot projection.
pub struct PostgresSessionStore {
    pool: PgPool,
    hot: Arc<dyn SessionStore>,
}

impl PostgresSessionStore {
    pub fn new(pool: PgPool, hot: Arc<dyn SessionStore>) -> Self {
        Self { pool, hot }
    }

    fn error(error: impl std::fmt::Display) -> CoreError {
        CoreError::Storage(format!("durable session checkpoint: {error}"))
    }

    fn encode(session: &Session) -> CoreResult<(i64, serde_json::Value)> {
        let version = i64::try_from(session.version).map_err(Self::error)?;
        let snapshot = serde_json::to_value(session).map_err(Self::error)?;
        Ok((version, snapshot))
    }

    async fn read(&self, id: &SessionId) -> CoreResult<Option<Session>> {
        let snapshot: Option<serde_json::Value> =
            sqlx::query_scalar("SELECT snapshot FROM session_checkpoints WHERE id=$1")
                .bind(&id.0)
                .fetch_optional(&self.pool)
                .await
                .map_err(Self::error)?;
        snapshot
            .map(serde_json::from_value)
            .transpose()
            .map_err(Self::error)
    }

    async fn project(&self, session: &Session) {
        // The commit already exists. Never make its acknowledgment depend on
        // a failed/paused Redis node, and never use this projection for CAS.
        match timeout(Duration::from_millis(100), self.hot.put(session)).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                warn!(session_id=%session.id, %error, "durable commit retained; Redis projection failed")
            }
            Err(_) => {
                warn!(session_id=%session.id, "durable commit retained; Redis projection timed out")
            }
        }
    }

    pub async fn recovery_sessions(&self) -> CoreResult<Vec<Session>> {
        let snapshots: Vec<serde_json::Value> = sqlx::query_scalar(
            "SELECT snapshot FROM session_checkpoints WHERE NOT archived ORDER BY updated_at",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(Self::error)?;
        snapshots
            .into_iter()
            .map(|value| serde_json::from_value(value).map_err(Self::error))
            .collect()
    }
}

#[async_trait]
impl SessionStore for PostgresSessionStore {
    async fn get(&self, id: &SessionId) -> CoreResult<Option<Session>> {
        if let Some(session) = self.read(id).await? {
            return Ok(Some(session));
        }

        // Adopt pre-upgrade active calls without overwriting concurrent commits.
        if let Some(legacy) = self.hot.get(id).await? {
            self.create_if_absent(&legacy).await?;
            return self.read(id).await;
        }
        Ok(None)
    }

    async fn create_if_absent(&self, session: &Session) -> CoreResult<bool> {
        let (version, snapshot) = Self::encode(session)?;
        let inserted = sqlx::query(
            "INSERT INTO session_checkpoints(id,version,snapshot,updated_at) VALUES($1,$2,$3,$4) ON CONFLICT(id) DO NOTHING",
        )
        .bind(&session.id.0).bind(version).bind(snapshot).bind(session.updated_at)
        .execute(&self.pool).await.map_err(Self::error)?.rows_affected() == 1;
        if inserted {
            self.project(session).await;
        }
        Ok(inserted)
    }

    async fn put(&self, session: &Session) -> CoreResult<()> {
        let (version, snapshot) = Self::encode(session)?;
        let affected = sqlx::query(
            "INSERT INTO session_checkpoints(id,version,snapshot,updated_at) VALUES($1,$2,$3,$4) ON CONFLICT(id) DO UPDATE SET version=EXCLUDED.version,snapshot=EXCLUDED.snapshot,updated_at=EXCLUDED.updated_at,archived=false WHERE session_checkpoints.version < EXCLUDED.version OR session_checkpoints.snapshot=EXCLUDED.snapshot",
        )
        .bind(&session.id.0).bind(version).bind(snapshot).bind(session.updated_at)
        .execute(&self.pool).await.map_err(Self::error)?.rows_affected();
        if affected == 0 {
            return Err(CoreError::VersionConflict {
                session_id: session.id.0.clone(),
                expected_version: session.version,
            });
        }
        self.project(session).await;
        Ok(())
    }

    async fn update_cas(&self, session: &Session, expected_version: u64) -> CoreResult<()> {
        if session.version <= expected_version {
            return Err(Self::error("CAS must advance the session version"));
        }
        let (version, snapshot) = Self::encode(session)?;
        let expected = i64::try_from(expected_version).map_err(Self::error)?;
        let affected = sqlx::query(
            "UPDATE session_checkpoints SET version=$2,snapshot=$3,updated_at=$4,archived=false WHERE id=$1 AND version=$5",
        )
        .bind(&session.id.0).bind(version).bind(snapshot).bind(session.updated_at).bind(expected)
        .execute(&self.pool).await.map_err(Self::error)?.rows_affected();
        if affected != 1 {
            return Err(CoreError::VersionConflict {
                session_id: session.id.0.clone(),
                expected_version,
            });
        }
        self.project(session).await;
        Ok(())
    }

    async fn delete_if_version(&self, id: &SessionId, expected_version: u64) -> CoreResult<bool> {
        let version = i64::try_from(expected_version).map_err(Self::error)?;
        let affected =
            sqlx::query("UPDATE session_checkpoints SET archived=true WHERE id=$1 AND version=$2")
                .bind(&id.0)
                .bind(version)
                .execute(&self.pool)
                .await
                .map_err(Self::error)?
                .rows_affected();
        if affected == 0 {
            return Ok(false);
        }
        match timeout(
            Duration::from_millis(100),
            self.hot.delete_if_version(id, expected_version),
        )
        .await
        {
            Ok(Ok(_)) => {}
            Ok(Err(error)) => {
                warn!(session_id=%id, %error, "archived checkpoint retained; Redis cleanup failed")
            }
            Err(_) => {
                warn!(session_id=%id, "archived checkpoint retained; Redis cleanup timed out")
            }
        }
        Ok(true)
    }

    async fn delete(&self, id: &SessionId) -> CoreResult<()> {
        // Administrative deletion is not part of archival.
        self.hot.delete(id).await?;
        sqlx::query("DELETE FROM session_checkpoints WHERE id=$1")
            .bind(&id.0)
            .execute(&self.pool)
            .await
            .map_err(Self::error)?;
        Ok(())
    }
}
