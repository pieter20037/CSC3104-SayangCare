use chrono::{DateTime, Utc};
use sayangcare_core::{CoreError, CoreResult};
use sqlx::PgPool;

/// Durable volunteer/session bindings shared by every API replica.
///
/// Read-only assignment queries. All mutations go through OwnershipStore.
pub struct VolunteerBindingStore {
    pool: PgPool,
}

#[derive(sqlx::FromRow)]
pub struct StaleVolunteerBinding {
    pub session_id: String,
    pub volunteer_id: String,
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
    ) -> CoreResult<Vec<StaleVolunteerBinding>> {
        sqlx::query_as::<_, StaleVolunteerBinding>(
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL; uses a connection-local temporary table"]
    async fn stale_bindings_keep_session_and_volunteer_distinct() {
        let url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL required");
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect(&url)
            .await
            .unwrap();
        sqlx::query(
            "CREATE TEMP TABLE volunteer_assignments (
                session_id text, volunteer_id text, status text,
                resolved_at timestamptz, updated_at timestamptz
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO volunteer_assignments VALUES
             ('SIM-stale', 'volunteer-noor', 'assigned', NULL, NOW() - INTERVAL '10 minutes'),
             ('SIM-recent', 'volunteer-aisha', 'assigned', NULL, NOW()),
             ('SIM-resolved', 'volunteer-noor', 'assigned', NOW(), NOW() - INTERVAL '10 minutes')",
        )
        .execute(&pool)
        .await
        .unwrap();

        let bindings = VolunteerBindingStore::new(pool.clone())
            .list_stale_before(Utc::now() - chrono::Duration::minutes(5))
            .await
            .unwrap();
        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings[0].session_id, "SIM-stale");
        assert_eq!(bindings[0].volunteer_id, "volunteer-noor");
        pool.close().await;
    }
}
