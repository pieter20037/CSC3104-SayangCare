use async_trait::async_trait;
use sayangcare_core::domain::{Session, SessionId, Transcript};
use sayangcare_core::ports::ArchiveStore;
use sayangcare_core::{CoreError, CoreResult};
use sqlx::PgPool;

pub struct PostgresArchiveStore {
    pool: PgPool,
}

impl PostgresArchiveStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl ArchiveStore for PostgresArchiveStore {
    async fn archive(&self, session: &Session) -> CoreResult<()> {
        let transcript_json = serde_json::to_value(&session.transcript)
            .map_err(|e| CoreError::Storage(format!("encode transcript: {e}")))?;
        let risk_json = serde_json::to_value(&session.risk)
            .map_err(|e| CoreError::Storage(format!("encode risk: {e}")))?;

        sqlx::query!(
            r#"
            INSERT INTO sessions
                (id, caller_phone, state, transcript, risk, created_at, updated_at, version)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            ON CONFLICT (id) DO UPDATE SET
                state = EXCLUDED.state,
                transcript = EXCLUDED.transcript,
                risk = EXCLUDED.risk,
                updated_at = EXCLUDED.updated_at,
                version = EXCLUDED.version
            "#,
            session.id.0,
            session.caller.phone_number,
            format!("{:?}", session.state).to_lowercase(),
            transcript_json,
            risk_json,
            session.created_at,
            session.updated_at,
            session.version as i64,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| CoreError::Storage(format!("pg archive: {e}")))?;

        Ok(())
    }

    async fn fetch_transcript(&self, id: &SessionId) -> CoreResult<Option<Transcript>> {
        let row = sqlx::query!(
            r#"SELECT transcript FROM sessions WHERE id = $1"#,
            id.0
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| CoreError::Storage(format!("pg fetch: {e}")))?;

        match row {
            Some(r) => {
                let t: Transcript = serde_json::from_value(r.transcript)
                    .map_err(|e| CoreError::Storage(format!("decode: {e}")))?;
                Ok(Some(t))
            }
            None => Ok(None),
        }
    }
}