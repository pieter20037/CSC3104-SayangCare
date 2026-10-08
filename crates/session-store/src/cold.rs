use async_trait::async_trait;
use sayangcare_core::domain::{CallerId, Session, SessionId, SessionState};
use sayangcare_core::ports::ArchiveStore;
use sayangcare_core::{CoreError, CoreResult};
use sqlx::{PgPool, Row};
use std::fmt::Display;

fn archive_decode_error(error: impl Display) -> CoreError {
    CoreError::Storage(format!("decode archived session: {error}"))
}

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
        let sentiment_json = session
            .latest_sentiment
            .as_ref()
            .map(serde_json::to_value)
            .transpose()
            .map_err(|e| CoreError::Storage(format!("encode sentiment: {e}")))?;

        sqlx::query(
            r#"
            INSERT INTO sessions
                (id, caller_phone, caller_display_name, state, transcript, risk,
                 latest_sentiment, handoff_status, assigned_volunteer_id,
                 escalation_count, operator_acknowledged, created_at, updated_at, version)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)
            ON CONFLICT (id) DO UPDATE SET
                caller_phone = EXCLUDED.caller_phone,
                caller_display_name = EXCLUDED.caller_display_name,
                state = EXCLUDED.state,
                transcript = EXCLUDED.transcript,
                risk = EXCLUDED.risk,
                latest_sentiment = EXCLUDED.latest_sentiment,
                handoff_status = EXCLUDED.handoff_status,
                assigned_volunteer_id = EXCLUDED.assigned_volunteer_id,
                escalation_count = EXCLUDED.escalation_count,
                operator_acknowledged = EXCLUDED.operator_acknowledged,
                updated_at = EXCLUDED.updated_at,
                version = EXCLUDED.version
            "#,
        )
        .bind(&session.id.0)
        .bind(&session.caller.phone_number)
        .bind(&session.caller.display_name)
        .bind(format!("{:?}", session.state).to_lowercase())
        .bind(transcript_json)
        .bind(risk_json)
        .bind(sentiment_json)
        .bind(format!("{:?}", session.handoff_status).to_lowercase())
        .bind(&session.assigned_volunteer_id)
        .bind(session.escalation_count as i32)
        .bind(session.operator_acknowledged)
        .bind(session.created_at)
        .bind(session.updated_at)
        .bind(session.version as i64)
        .execute(&self.pool)
        .await
        .map_err(|e| CoreError::Storage(format!("pg archive: {e}")))?;

        Ok(())
    }

    async fn fetch_session(&self, id: &SessionId) -> CoreResult<Option<Session>> {
        let row = sqlx::query(
            r#"SELECT id, caller_phone, caller_display_name, state, transcript, risk,
                      latest_sentiment, handoff_status, assigned_volunteer_id,
                      escalation_count, operator_acknowledged, created_at, updated_at,
                      version
               FROM sessions WHERE id = $1"#,
        )
        .bind(&id.0)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| CoreError::Storage(format!("pg fetch: {e}")))?;

        match row {
            Some(r) => {
                let state: String = r.try_get("state").map_err(archive_decode_error)?;
                let state: SessionState = serde_json::from_value(serde_json::Value::String(state))
                    .map_err(archive_decode_error)?;
                let transcript =
                    serde_json::from_value(r.try_get("transcript").map_err(archive_decode_error)?)
                        .map_err(archive_decode_error)?;
                let risk = serde_json::from_value(r.try_get("risk").map_err(archive_decode_error)?)
                    .map_err(archive_decode_error)?;
                let latest_sentiment = r
                    .try_get::<Option<serde_json::Value>, _>("latest_sentiment")
                    .map_err(archive_decode_error)?
                    .map(serde_json::from_value)
                    .transpose()
                    .map_err(archive_decode_error)?;
                let handoff_status: String = r
                    .try_get("handoff_status")
                    .unwrap_or_else(|_| "not_required".to_string());
                let handoff_status = match handoff_status.as_str() {
                    "pending" => sayangcare_core::domain::HandoffStatus::Pending,
                    "assigned" => sayangcare_core::domain::HandoffStatus::Assigned,
                    "transferred" => sayangcare_core::domain::HandoffStatus::Transferred,
                    "resolved" => sayangcare_core::domain::HandoffStatus::Resolved,
                    _ => sayangcare_core::domain::HandoffStatus::NotRequired,
                };
                let assigned_volunteer_id: Option<String> = r.try_get("assigned_volunteer_id").ok();
                let escalation_count: i32 = r.try_get("escalation_count").unwrap_or_default();
                let operator_acknowledged: bool =
                    r.try_get("operator_acknowledged").unwrap_or(false);
                let version: i64 = r.try_get("version").map_err(archive_decode_error)?;
                let version = u64::try_from(version).map_err(archive_decode_error)?;

                Ok(Some(Session {
                    id: SessionId(r.try_get("id").map_err(archive_decode_error)?),
                    is_simulated: false,
                    caller: CallerId {
                        phone_number: r.try_get("caller_phone").map_err(archive_decode_error)?,
                        display_name: r
                            .try_get("caller_display_name")
                            .map_err(archive_decode_error)?,
                    },
                    state,
                    transcript,
                    latest_sentiment,
                    risk,
                    handoff_status,
                    assigned_volunteer_id,
                    escalation_count: u8::try_from(escalation_count).unwrap_or_default(),
                    operator_acknowledged,
                    created_at: r.try_get("created_at").map_err(archive_decode_error)?,
                    updated_at: r.try_get("updated_at").map_err(archive_decode_error)?,
                    version,
                }))
            }
            None => Ok(None),
        }
    }
}
