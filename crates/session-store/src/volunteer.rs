use sayangcare_core::{CoreError, CoreResult};
use sqlx::PgPool;

/// Durable volunteer/session bindings shared by every API replica.
///
/// The session primary key makes claims idempotent and PostgreSQL's unique row
/// arbitration prevents two volunteers from owning the same session. Each
/// operation is one indexed SQL statement: O(1) average with the primary key.
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
            ON CONFLICT (session_id) DO NOTHING
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
        let updated = sqlx::query_scalar::<_, String>(
            r#"
            UPDATE volunteer_assignments
            SET volunteer_id = $3, status = 'transferred', updated_at = now()
            WHERE session_id = $1
              AND volunteer_id = $2
              AND status IN ('assigned', 'transferred')
            RETURNING session_id
            "#,
        )
        .bind(session_id)
        .bind(from_volunteer_id)
        .bind(to_volunteer_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| CoreError::Storage(format!("transfer volunteer binding: {e}")))?;

        Ok(updated.is_some())
    }

    pub async fn release(&self, session_id: &str, volunteer_id: &str) -> CoreResult<bool> {
        let updated = sqlx::query_scalar::<_, String>(
            r#"
            UPDATE volunteer_assignments
            SET status = 'resolved', resolved_at = now(), updated_at = now()
            WHERE session_id = $1
              AND volunteer_id = $2
              AND status IN ('assigned', 'transferred')
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
}
