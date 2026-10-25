use super::*;
use sayangcare_core::domain::{CallerId, SessionState, Speaker, Turn};

struct UnavailableHot;

#[async_trait]
impl SessionStore for UnavailableHot {
    async fn get(&self, _: &SessionId) -> CoreResult<Option<Session>> {
        Err(CoreError::Storage("injected Redis outage".into()))
    }
    async fn create_if_absent(&self, _: &Session) -> CoreResult<bool> {
        Err(CoreError::Storage("injected Redis outage".into()))
    }
    async fn put(&self, _: &Session) -> CoreResult<()> {
        Err(CoreError::Storage("injected Redis outage".into()))
    }
    async fn update_cas(&self, _: &Session, _: u64) -> CoreResult<()> {
        Err(CoreError::Storage("injected Redis outage".into()))
    }
    async fn delete(&self, _: &SessionId) -> CoreResult<()> {
        Err(CoreError::Storage("injected Redis outage".into()))
    }
    async fn delete_if_version(&self, _: &SessionId, _: u64) -> CoreResult<bool> {
        Err(CoreError::Storage("injected Redis outage".into()))
    }
}

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL; creates and removes an isolated PostgreSQL schema"]
async fn durable_commit_survives_redis_loss_and_fences_concurrent_updates() {
    let url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL is required");
    let admin = PgPool::connect(&url).await.unwrap();
    let schema = format!(
        "checkpoint_test_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .unwrap();
    let search_path = schema.clone();
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(3)
        .after_connect(move |connection, _| {
            let statement = format!("SET search_path TO {search_path}");
            Box::pin(async move {
                sqlx::query(&statement).execute(connection).await?;
                Ok(())
            })
        })
        .connect(&url)
        .await
        .unwrap();
    sqlx::raw_sql(include_str!(
        "../../../migrations/0010_session_checkpoints.sql"
    ))
    .execute(&pool)
    .await
    .unwrap();
    let store = PostgresSessionStore::new(pool.clone(), Arc::new(UnavailableHot));
    let mut session = Session::new(CallerId {
        phone_number: "+6500000000".into(),
        display_name: None,
    });
    session.id = SessionId("SIM-DURABLE".into());
    assert!(store.create_if_absent(&session).await.unwrap());
    assert!(!store.create_if_absent(&session).await.unwrap());
    assert_eq!(
        store.get(&session.id).await.unwrap().unwrap().version,
        session.version
    );
    let mut next = session.clone();
    next.transition(SessionState::Active).unwrap();
    next.record_turns([Turn {
        speaker: Speaker::Caller,
        text: "acknowledged text".into(),
        timestamp: chrono::Utc::now(),
    }])
    .unwrap();
    let (first, second) = tokio::join!(
        store.update_cas(&next, session.version),
        store.update_cas(&next, session.version)
    );
    assert_eq!(usize::from(first.is_ok()) + usize::from(second.is_ok()), 1);
    assert_eq!(
        store
            .get(&session.id)
            .await
            .unwrap()
            .unwrap()
            .transcript
            .turns
            .len(),
        1
    );
    assert!(
        store.put(&session).await.is_err(),
        "stale snapshot must not overwrite a durable turn"
    );
    assert!(!store
        .delete_if_version(&session.id, session.version)
        .await
        .unwrap());
    assert_eq!(store.recovery_sessions().await.unwrap().len(), 1);
    assert!(store
        .delete_if_version(&next.id, next.version)
        .await
        .unwrap());
    assert!(store.recovery_sessions().await.unwrap().is_empty());
    assert!(
        store.get(&next.id).await.unwrap().is_some(),
        "archival must retain the commit authority"
    );
    pool.close().await;
    assert!(
        store.create_if_absent(&session).await.is_err(),
        "failed durable storage cannot acknowledge a mutation"
    );
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
}
