use sqlx::{postgres::PgPoolOptions, PgConnection, Postgres};
use std::time::Duration;

async fn is_writable_primary(connection: &mut PgConnection) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT NOT pg_is_in_recovery() AND current_setting('transaction_read_only') = 'off'",
    )
    .fetch_one(connection)
    .await
}

/// Discard dead or demoted connections before application work starts, not by
/// replaying a write whose commit acknowledgment may have been lost.
pub fn primary_pool_options(max_connections: u32) -> sqlx::pool::PoolOptions<Postgres> {
    PgPoolOptions::new()
        .max_connections(max_connections)
        .acquire_timeout(Duration::from_secs(2))
        // The primary probe also checks liveness; avoid a second round trip.
        .test_before_acquire(false)
        .after_connect(|connection, _| {
            Box::pin(async move {
                if is_writable_primary(connection).await? {
                    Ok(())
                } else {
                    tracing::warn!("rejecting new PostgreSQL connection: server is not writable");
                    Err(sqlx::Error::Protocol(
                        "PostgreSQL connection is not a writable primary".into(),
                    ))
                }
            })
        })
        .before_acquire(|connection, _| {
            Box::pin(async move {
                let writable = is_writable_primary(connection).await?;
                if !writable {
                    tracing::warn!(
                        "discarding pooled PostgreSQL connection to a non-writable server"
                    );
                }
                Ok(writable)
            })
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primary_validation_preserves_pool_limits() {
        let options = primary_pool_options(3);
        assert_eq!(options.get_max_connections(), 3);
        assert_eq!(options.get_acquire_timeout(), Duration::from_secs(2));
        assert!(!options.get_test_before_acquire());
    }

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL to an isolated writable PostgreSQL test server"]
    async fn checkout_replaces_read_only_and_terminated_connections() {
        let url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL required");
        let admin = sqlx::PgPool::connect(&url).await.unwrap();
        let pool = primary_pool_options(1).connect(&url).await.unwrap();
        let mut connection = pool.acquire().await.unwrap();
        let first: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *connection)
            .await
            .unwrap();
        sqlx::query("SET default_transaction_read_only = on")
            .execute(&mut *connection)
            .await
            .unwrap();
        assert!(!is_writable_primary(&mut connection).await.unwrap());
        drop(connection);
        let mut replacement = pool.acquire().await.unwrap();
        let second: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *replacement)
            .await
            .unwrap();
        assert_ne!(first, second);
        assert!(is_writable_primary(&mut replacement).await.unwrap());
        drop(replacement);
        let terminated: bool = sqlx::query_scalar("SELECT pg_terminate_backend($1)")
            .bind(second)
            .fetch_one(&admin)
            .await
            .unwrap();
        assert!(terminated);
        let mut replacement = pool.acquire().await.unwrap();
        let third: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *replacement)
            .await
            .unwrap();
        assert_ne!(second, third);
        assert!(is_writable_primary(&mut replacement).await.unwrap());
        drop(replacement);
        pool.close().await;
        admin.close().await;
    }
}
