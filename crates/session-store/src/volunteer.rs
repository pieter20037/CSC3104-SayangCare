use chrono::{DateTime, Utc};
use sayangcare_core::{CoreError, CoreResult};
use sqlx::PgPool;

/// Durable volunteer/session bindings shared by every API replica.
///
/// A partial unique index arbitrates active ownership while resolved assignments
/// remain as audit history. Each operation is indexed and executes transactionally.
pub struct VolunteerBindingStore {
    pool: PgPool,
}

impl VolunteerBindingStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn claim(
        &self,
        session_id: &str,
        volunteer_id: &str,
        risk_level: u8,
    ) -> CoreResult<bool> {
        let inserted = sqlx::query_scalar::<_, String>(
            r#"
            INSERT INTO volunteer_assignments
                (session_id, volunteer_id, risk_level, status)
            VALUES ($1, $2, $3, 'assigned')
            ON CONFLICT DO NOTHING
            RETURNING session_id
            "#,
        )
        .bind(session_id)
        .bind(volunteer_id)
        .bind(i16::from(risk_level))
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| CoreError::Storage(format!("claim volunteer binding: {e}")))?;

        Ok(inserted.is_some())
    }

    pub async fn transfer(
        &self,
        session_id: &str,
        from_volunteer_id: &str,
        to_volunteer_id: &str,
    ) -> CoreResult<bool> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| CoreError::Storage(format!("begin volunteer transfer: {e}")))?;
        let risk_level = sqlx::query_scalar::<_, i16>(
            r#"
            UPDATE volunteer_assignments
            SET status = 'transferred', resolved_at = now(), updated_at = now()
            WHERE session_id = $1
              AND volunteer_id = $2
              AND status IN ('assigned', 'transferred')
              AND resolved_at IS NULL
            RETURNING risk_level
            "#,
        )
        .bind(session_id)
        .bind(from_volunteer_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|e| CoreError::Storage(format!("transfer volunteer binding: {e}")))?;

        let Some(risk_level) = risk_level else {
            tx.rollback()
                .await
                .map_err(|e| CoreError::Storage(format!("rollback volunteer transfer: {e}")))?;
            return Ok(false);
        };

        sqlx::query(
            r#"
            INSERT INTO volunteer_assignments
                (session_id, volunteer_id, risk_level, status)
            VALUES ($1, $2, $3, 'assigned')
            "#,
        )
        .bind(session_id)
        .bind(to_volunteer_id)
        .bind(risk_level)
        .execute(&mut *tx)
        .await
        .map_err(|e| CoreError::Storage(format!("insert transferred volunteer binding: {e}")))?;
        tx.commit()
            .await
            .map_err(|e| CoreError::Storage(format!("commit volunteer transfer: {e}")))?;

        Ok(true)
    }

    pub async fn release(&self, session_id: &str, volunteer_id: &str) -> CoreResult<bool> {
        let updated = sqlx::query_scalar::<_, String>(
            r#"
            UPDATE volunteer_assignments
            SET status = 'resolved', resolved_at = now(), updated_at = now()
                        WHERE session_id = $1
              AND volunteer_id = $2
              AND status IN ('assigned', 'transferred')
                            AND resolved_at IS NULL
            RETURNING session_id
            "#,
        )
        .bind(session_id)
        .bind(volunteer_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| CoreError::Storage(format!("release volunteer binding: {e}")))?;

        Ok(updated.is_some())
    }

    pub async fn list_active_for_volunteer(&self, volunteer_id: &str) -> CoreResult<Vec<String>> {
        sqlx::query_scalar::<_, String>(
            r#"
            SELECT session_id
            FROM volunteer_assignments
            WHERE volunteer_id = $1
              AND status IN ('assigned', 'transferred')
              AND resolved_at IS NULL
            ORDER BY assigned_at, session_id
            "#,
        )
        .bind(volunteer_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| CoreError::Storage(format!("list volunteer bindings: {e}")))
    }

    pub async fn list_stale_before(
        &self,
        cutoff: DateTime<Utc>,
    ) -> CoreResult<Vec<(String, String)>> {
        sqlx::query_as::<_, (String, String)>(
            r#"
            SELECT session_id, volunteer_id
            FROM volunteer_assignments
            WHERE status IN ('assigned', 'transferred')
              AND resolved_at IS NULL
              AND updated_at <= $1
            ORDER BY updated_at, session_id
            "#,
        )
        .bind(cutoff)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| CoreError::Storage(format!("list stale volunteer bindings: {e}")))
    }
}
