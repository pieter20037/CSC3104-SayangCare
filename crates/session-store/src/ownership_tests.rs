use super::*;
use async_trait::async_trait;
use sayangcare_core::domain::{CallerId, SessionId};
use sayangcare_core::{CoreError, CoreResult};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::Mutex;

#[derive(Default)]
struct TestCoordinator {
    state: Mutex<(
        BTreeMap<String, AssignmentOwnership>,
        BTreeMap<String, OwnershipDecision>,
    )>,
    lose_response: AtomicBool,
}

#[async_trait]
impl OwnershipCoordinator for TestCoordinator {
    async fn commit(&self, command: OwnershipCommand) -> CoreResult<OwnershipDecision> {
        let mut state = self.state.lock().await;
        if let Some(result) = state.1.get(&command.request_id) {
            return Ok(result.clone());
        }
        let current = state.0.entry(command.session_id.0.clone()).or_default();
        let decision = current.decide(&command);
        if let OwnershipDecision::Applied { ownership } = &decision {
            *current = ownership.clone();
        }
        state.1.insert(command.request_id, decision.clone());
        if self.lose_response.swap(false, Ordering::SeqCst) {
            return Err(CoreError::Storage("committed, response lost".into()));
        }
        Ok(decision)
    }
}

fn command(
    id: &str,
    request: &str,
    generation: u64,
    transition: OwnershipTransition,
) -> OwnershipCommand {
    OwnershipCommand {
        request_id: request.into(),
        session_id: SessionId(id.into()),
        expected_generation: generation,
        transition,
    }
}

async fn seed(pool: &PgPool, id: &str) {
    let mut session = Session::with_id(
        SessionId(id.into()),
        CallerId {
            phone_number: "+6500000000".into(),
            display_name: None,
        },
    );
    session.transition(SessionState::Active).unwrap();
    session.transition(SessionState::Escalated).unwrap();
    sqlx::query(
        "INSERT INTO session_checkpoints(id,version,snapshot,updated_at) VALUES($1,$2,$3,now())",
    )
    .bind(id)
    .bind(i64::try_from(session.version).unwrap())
    .bind(serde_json::to_value(session).unwrap())
    .execute(pool)
    .await
    .unwrap();
}

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL; uses an isolated PostgreSQL schema"]
async fn transactional_fencing_and_ambiguous_commit_recovery() {
    let url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL required");
    let admin = PgPool::connect(&url).await.unwrap();
    let schema = format!("fencing_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .unwrap();
    let path = schema.clone();
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(5)
        .after_connect(move |conn, _| {
            let statement = format!("SET search_path TO {path}");
            Box::pin(async move {
                sqlx::query(&statement).execute(conn).await?;
                Ok(())
            })
        })
        .connect(&url)
        .await
        .unwrap();
    sqlx::migrate!("../../migrations").run(&pool).await.unwrap();
    let pg = OwnershipStore::new(pool.clone(), None);
    seed(&pool, "race").await;
    let a = command(
        "race",
        "claim-a",
        0,
        OwnershipTransition::Claim {
            volunteer_id: "a".into(),
        },
    );
    let b = command(
        "race",
        "claim-b",
        0,
        OwnershipTransition::Claim {
            volunteer_id: "b".into(),
        },
    );
    let (a, b) = tokio::join!(pg.execute(a), pg.execute(b));
    assert_eq!(
        usize::from(matches!(a, Ok(OwnershipDecision::Applied { .. })))
            + usize::from(matches!(b, Ok(OwnershipDecision::Applied { .. }))),
        1
    );
    let winner = pg.current("race").await.unwrap().volunteer_id.unwrap();
    let transfer = command(
        "race",
        "transfer",
        1,
        OwnershipTransition::Transfer {
            from_volunteer_id: winner.clone(),
            to_volunteer_id: "c".into(),
        },
    );
    let transferred = pg.execute(transfer.clone()).await.unwrap();
    assert!(matches!(
        transferred,
        OwnershipDecision::Applied {
            ownership: AssignmentOwnership {
                assignment_generation: 2,
                ..
            }
        }
    ));
    assert_eq!(pg.execute(transfer).await.unwrap(), transferred);
    assert!(matches!(
        pg.add_note("race", &winner, 1, "stale", "must not persist")
            .await,
        Err(OwnershipError::Stale)
    ));
    assert!(matches!(
        pg.execute(command(
            "race",
            "stale-release",
            1,
            OwnershipTransition::Release {
                volunteer_id: winner.clone(),
                resolved: false
            }
        ))
        .await,
        Err(OwnershipError::Stale)
    ));
    assert!(matches!(
        pg.execute(command(
            "race",
            "stale-transfer",
            1,
            OwnershipTransition::Transfer {
                from_volunteer_id: winner,
                to_volunteer_id: "d".into()
            }
        ))
        .await,
        Err(OwnershipError::Stale)
    ));
    pg.add_note("race", "c", 2, "valid", "current owner")
        .await
        .unwrap();
    pg.add_note("race", "c", 2, "valid", "current owner")
        .await
        .unwrap();
    assert!(matches!(
        pg.add_note("race", "c", 2, "valid", "changed payload")
            .await,
        Err(OwnershipError::Conflict)
    ));
    pg.execute(command(
        "race",
        "release",
        2,
        OwnershipTransition::Release {
            volunteer_id: "c".into(),
            resolved: false,
        },
    ))
    .await
    .unwrap();
    pg.execute(command(
        "race",
        "reclaim",
        3,
        OwnershipTransition::Claim {
            volunteer_id: "a".into(),
        },
    ))
    .await
    .unwrap();
    assert_eq!(pg.current("race").await.unwrap().assignment_generation, 4);
    assert!(matches!(
        pg.add_note("race", "a", 1, "aba", "old A token").await,
        Err(OwnershipError::Stale)
    ));
    let notes: i64 =
        sqlx::query_scalar("SELECT count(*) FROM volunteer_case_notes WHERE session_id='race'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(notes, 1);
    let active:i64 = sqlx::query_scalar("SELECT count(*) FROM volunteer_assignments WHERE session_id='race' AND resolved_at IS NULL").fetch_one(&pool).await.unwrap();
    assert_eq!(active, 1);

    seed(&pool, "ambiguous").await;
    let coordinator = Arc::new(TestCoordinator::default());
    coordinator.lose_response.store(true, Ordering::SeqCst);
    let raft = OwnershipStore::new(pool.clone(), Some(coordinator.clone()));
    let claim = command(
        "ambiguous",
        "ambiguous-claim",
        0,
        OwnershipTransition::Claim {
            volunteer_id: "a".into(),
        },
    );
    assert!(matches!(
        raft.execute(claim.clone()).await,
        Err(OwnershipError::Unavailable(_))
    ));
    assert!(raft
        .current("ambiguous")
        .await
        .unwrap()
        .pending_operation_id
        .is_some());
    // A restart uses the journal and the original coordinator request ID.
    let restarted = OwnershipStore::new(pool.clone(), Some(coordinator));
    restarted.recover().await.unwrap();
    assert_eq!(
        restarted
            .current("ambiguous")
            .await
            .unwrap()
            .assignment_generation,
        1
    );
    assert!(matches!(
        restarted.execute(claim).await.unwrap(),
        OwnershipDecision::Applied { .. }
    ));
    // Inject a database installation failure AFTER the coordinator commits.
    sqlx::raw_sql("CREATE FUNCTION fail_install() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.session_id='ambiguous' AND NEW.assignment_generation=2 THEN RAISE EXCEPTION 'injected install failure'; END IF; RETURN NEW; END $$; CREATE TRIGGER fail_install BEFORE UPDATE ON case_ownership FOR EACH ROW EXECUTE FUNCTION fail_install();")
        .execute(&pool).await.unwrap();
    let transfer = command(
        "ambiguous",
        "install-failure",
        1,
        OwnershipTransition::Transfer {
            from_volunteer_id: "a".into(),
            to_volunteer_id: "b".into(),
        },
    );
    assert!(matches!(
        restarted.execute(transfer.clone()).await,
        Err(OwnershipError::Unavailable(_))
    ));
    assert!(matches!(
        restarted
            .add_note("ambiguous", "a", 1, "blocked", "must be blocked")
            .await,
        Err(OwnershipError::Pending)
    ));
    sqlx::raw_sql("DROP TRIGGER fail_install ON case_ownership; DROP FUNCTION fail_install();")
        .execute(&pool)
        .await
        .unwrap();
    restarted.recover().await.unwrap();
    let recovered = restarted.current("ambiguous").await.unwrap();
    assert_eq!(recovered.assignment_generation, 2);
    assert_eq!(recovered.volunteer_id.as_deref(), Some("b"));
    assert!(recovered.pending_operation_id.is_none());
    restarted
        .add_note("ambiguous", "b", 2, "after-recovery", "allowed")
        .await
        .unwrap();
    assert!(matches!(
        restarted
            .add_note("ambiguous", "a", 1, "rejected", "stale")
            .await,
        Err(OwnershipError::Stale)
    ));
    let snapshot: serde_json::Value =
        sqlx::query_scalar("SELECT snapshot FROM session_checkpoints WHERE id='ambiguous'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(snapshot["assignment_generation"], 2);
    assert_eq!(snapshot["assigned_volunteer_id"], "b");
    // Mode switching on retained generations is explicitly prohibited.
    assert!(matches!(
        pg.execute(command(
            "ambiguous",
            "bad-mode",
            2,
            OwnershipTransition::Release {
                volunteer_id: "b".into(),
                resolved: true
            }
        ))
        .await,
        Err(OwnershipError::Invalid(_))
    ));
    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
}
