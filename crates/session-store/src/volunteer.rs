use chrono::{DateTime, Utc};
use sayangcare_core::{CoreError, CoreResult};
use sqlx::PgPool;

/// Durable volunteer/session bindings shared by every API replica.
///
/// Read-only assignment queries. All mutations go through OwnershipStore.
pub struct VolunteerBindingStore {
    pool: PgPool,
}

impl VolunteerBindingStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
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
