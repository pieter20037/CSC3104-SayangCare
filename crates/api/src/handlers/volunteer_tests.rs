use super::*;
use actix_web::{http::StatusCode, test, App};
use serde_json::json;

#[actix_web::test]
async fn protected_actions_require_explicit_generation_and_request_id() {
    let app = test::init_service(App::new().route(
        "/",
        web::post().to(|_: web::Json<CaseAction>| async { HttpResponse::Ok().finish() }),
    ))
    .await;
    for body in [
        json!({}),
        json!({"request_id":"r"}),
        json!({"assignment_generation":1}),
        json!({"assignment_generation":-1,"request_id":"r"}),
    ] {
        let response = test::call_service(
            &app,
            test::TestRequest::post()
                .uri("/")
                .set_json(body)
                .to_request(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
    let response = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/")
            .set_json(json!({"assignment_generation":1,"request_id":"r"}))
            .to_request(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        ownership_error(OwnershipError::Stale).status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        ownership_error(OwnershipError::Pending).status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
}

#[actix_web::test]
#[ignore = "requires isolated TEST_DATABASE_URL and TEST_REDIS_URL"]
async fn http_claim_race_transfer_notes_and_stale_owner_are_fenced() {
    let base = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL");
    let redis = std::env::var("TEST_REDIS_URL").expect("TEST_REDIS_URL");
    let admin = sqlx::PgPool::connect(&base).await.unwrap();
    let schema = format!("fencing_api_{}", Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .unwrap();
    let separator = if base.contains('?') { "&" } else { "?" };
    let url = format!("{base}{separator}options=-csearch_path%3D{schema}");
    let config = serde_json::from_value(serde_json::json!({
        "server":{"host":"127.0.0.1","port":0,"workers":1},
        "redis":{"master_url":redis,"sentinel_endpoints":[],"master_name":"test","password":null,"pool_size":3},
        "postgres":{"url":url,"max_connections":5},
        "circuit_breaker":{"key_prefix":format!("fencing-test-{schema}"),"base_error_rate":0.25,"base_latency_ms":2000,"min_requests":20,"half_open_after_secs":10},
        "telephony":{"twilio_account_sid":"test","twilio_auth_token":"test","twilio_from_number":null,"public_base_url":"http://127.0.0.1"}
    })).unwrap();
    let state = Arc::new(AppState::bootstrap(config).await.unwrap());
    let mut session = Session::with_id(
        SessionId(format!("SIM-{schema}")),
        sayangcare_core::domain::CallerId {
            phone_number: "+6500000000".into(),
            display_name: None,
        },
    );
    session.is_simulated = true;
    session.transition(SessionState::Active).unwrap();
    session.transition(SessionState::Escalated).unwrap();
    state.sessions.create_if_absent(&session).await.unwrap();
    state
        .queue
        .enqueue(session.id.clone(), 5, chrono::Utc::now())
        .await
        .unwrap();
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(state.clone()))
            .route("/claim", web::post().to(claim))
            .route("/{volunteer}/{id}/transfer", web::post().to(transfer_case))
            .route("/{volunteer}/{id}/release", web::post().to(release_case))
            .route("/{volunteer}/{id}/resolve", web::post().to(resolve_case))
            .route("/{volunteer}/{id}/notes", web::post().to(add_note)),
    )
    .await;
    let id = &session.id.0;
    let a = test::TestRequest::post().uri("/claim").set_json(json!({"session_id":id,"volunteer_id":"volunteer-aisha","request_id":format!("{id}-a")})).to_request();
    let b = test::TestRequest::post()
        .uri("/claim")
        .set_json(
            json!({"session_id":id,"volunteer_id":"volunteer-noor","request_id":format!("{id}-b")}),
        )
        .to_request();
    let (a, b) = tokio::join!(test::call_service(&app, a), test::call_service(&app, b));
    let statuses = [a.status(), b.status()];
    assert_eq!(statuses.iter().filter(|s| **s == StatusCode::OK).count(), 1);
    assert_eq!(
        statuses
            .iter()
            .filter(|s| **s == StatusCode::CONFLICT)
            .count(),
        1
    );
    let current = state.ownership.current(id).await.unwrap();
    assert_eq!(current.assignment_generation, 1);
    let owner = current.volunteer_id.unwrap();
    let target = if owner == "volunteer-aisha" {
        "volunteer-noor"
    } else {
        "volunteer-aisha"
    };
    let response=test::call_service(&app,test::TestRequest::post().uri(&format!("/{owner}/{id}/transfer"))
        .set_json(json!({"assignment_generation":1,"request_id":format!("{id}-transfer"),"to_volunteer_id":target})).to_request()).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body: serde_json::Value = test::read_body_json(response).await;
    assert_eq!(body["assignment_generation"], 2);
    for action in ["notes", "transfer", "release", "resolve"] {
        let response=test::call_service(&app,test::TestRequest::post().uri(&format!("/{owner}/{id}/{action}"))
            .set_json(json!({"assignment_generation":1,"request_id":format!("{id}-stale-{action}"),"body":"must not persist"})).to_request()).await;
        assert_eq!(response.status(), StatusCode::CONFLICT, "{action}");
        let body: serde_json::Value = test::read_body_json(response).await;
        assert_eq!(
            body["error"],
            "Stale assignment generation (fencing token rejected)"
        );
    }
    let response=test::call_service(&app,test::TestRequest::post().uri(&format!("/{target}/{id}/notes"))
        .set_json(json!({"assignment_generation":2,"request_id":format!("{id}-valid"),"body":"current owner note"})).to_request()).await;
    assert_eq!(response.status(), StatusCode::OK);
    let notes: i64 =
        sqlx::query_scalar("SELECT count(*) FROM volunteer_case_notes WHERE session_id=$1")
            .bind(id)
            .fetch_one(&state.postgres)
            .await
            .unwrap();
    assert_eq!(notes, 1);
    state.postgres.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
}
