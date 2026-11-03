use super::*;
use sayangcare_core::domain::{CallerId, SessionState, Speaker, Turn};

#[test]
fn creation_retries_only_transient_connection_failures() {
    assert!(retryable_creation_error(&sqlx::Error::PoolTimedOut));
    assert!(retryable_creation_error(&sqlx::Error::Io(
        std::io::Error::from(std::io::ErrorKind::ConnectionReset),
    )));
    assert!(!retryable_creation_error(&sqlx::Error::PoolClosed));
    assert!(!retryable_creation_error(&sqlx::Error::Protocol(
        "invalid protocol".into(),
    )));
    assert!(!retryable_creation_error(&sqlx::Error::RowNotFound));
}

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
        .acquire_timeout(Duration::from_secs(2))
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
    sqlx::migrate!("../../migrations").run(&pool).await.unwrap();
    let store = PostgresSessionStore::new(pool.clone(), Arc::new(UnavailableHot));
    let mut session = Session::new(CallerId {
        phone_number: "+6500000000".into(),
        display_name: None,
    });
    session.id = SessionId("SIM-DURABLE".into());
    assert!(store.create_if_absent(&session).await.unwrap());
    assert!(!store.create_if_absent(&session).await.unwrap());
    let held = vec![
        pool.acquire().await.unwrap(),
        pool.acquire().await.unwrap(),
        pool.acquire().await.unwrap(),
    ];
    let (replayed, ()) = tokio::join!(store.create_if_absent(&session), async move {
        tokio::time::sleep(Duration::from_millis(2200)).await;
        drop(held);
    });
    assert!(
        !replayed.unwrap(),
        "recovery must not duplicate an existing SID"
    );
    let held = vec![
        pool.acquire().await.unwrap(),
        pool.acquire().await.unwrap(),
        pool.acquire().await.unwrap(),
    ];
    let started = tokio::time::Instant::now();
    assert!(matches!(
        store.create_if_absent(&session).await,
        Err(CoreError::StorageTransient(_))
    ));
    assert!(started.elapsed() >= Duration::from_secs(5));
    assert!(started.elapsed() < Duration::from_millis(5500));
    drop(held);
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
    let mut atomic = Session::with_id(
        SessionId("SIM-ATOMIC-FINISH".into()),
        session.caller.clone(),
    );
    atomic.transition(SessionState::Active).unwrap();
    assert!(store.create_if_absent(&atomic).await.unwrap());
    sqlx::query("ALTER TABLE sessions ADD CONSTRAINT injected_archive_failure CHECK (id <> 'SIM-ATOMIC-FINISH')")
        .execute(&pool).await.unwrap();
    assert!(store
        .finish_and_archive(&atomic.id, SessionState::Completed)
        .await
        .is_err());
    let retained = store.get(&atomic.id).await.unwrap().unwrap();
    assert_eq!(retained.version, atomic.version);
    assert_eq!(retained.state, SessionState::Active);
    assert_eq!(store.recovery_sessions().await.unwrap().len(), 1);
    sqlx::query("ALTER TABLE sessions DROP CONSTRAINT injected_archive_failure")
        .execute(&pool)
        .await
        .unwrap();
    let (first, replay) = tokio::join!(
        store.finish_and_archive(&atomic.id, SessionState::Completed),
        store.finish_and_archive(&atomic.id, SessionState::Completed)
    );
    assert!(first.unwrap() && replay.unwrap());
    let completed = store.get(&atomic.id).await.unwrap().unwrap();
    assert_eq!(completed.version, atomic.version + 1);
    assert_eq!(completed.state, SessionState::Completed);
    assert!(store.recovery_sessions().await.unwrap().is_empty());
    let archived = crate::PostgresArchiveStore::new(pool.clone());
    use sayangcare_core::ports::ArchiveStore;
    assert_eq!(
        archived
            .fetch_session(&atomic.id)
            .await
            .unwrap()
            .unwrap()
            .version,
        completed.version
    );
    let mut stale = atomic.clone();
    stale.transition(SessionState::Completed).unwrap();
    assert!(store.update_cas(&stale, atomic.version).await.is_err());
    assert!(store
        .finish_and_archive(&atomic.id, SessionState::Active)
        .await
        .is_err());
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
