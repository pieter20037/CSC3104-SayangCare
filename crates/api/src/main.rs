//! SayangCare API binary entry point.
//!
//! Responsibilities:
//!   1. Initialise logging/telemetry.
//!   2. Load configuration from config/*.toml + environment.
//!   3. Bootstrap AppState (Redis, Postgres, breakers, queue, telephony).
//!   4. Spawn background tasks (breaker ticker, archive flusher, shutdown hooks).
//!   5. Start Actix Web with graceful shutdown.

use actix_web::{middleware::Compress, web, App, HttpServer};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};
use tracing_actix_web::TracingLogger;

mod fallback;
mod handlers;
mod handoff_recovery;
mod inference;
mod middleware;
mod perf_metrics;
mod performance;
mod sentiment;
mod state;
mod telemetry;

use crate::state::AppState;

#[actix_web::main]
async fn main() -> anyhow::Result<()> {
    // 1. Load .env (dev only; in k8s, env vars are injected directly).
    dotenvy::dotenv().ok();

    // 2. Init logging early so config errors are visible.
    telemetry::init();

    // 3. Load config.
    let config = match sayangcare_core::config::AppConfig::load() {
        Ok(c) => c,
        Err(e) => {
            // tracing may not be initialised enough to log structured errors,
            // so fall back to eprintln for this fatal case.
            eprintln!("FATAL: failed to load config: {e}");
            std::process::exit(1);
        }
    };

    info!(
        host = %config.server.host,
        port = config.server.port,
        workers = config.server.workers,
        "SayangCare starting"
    );

    // 4. Bootstrap AppState. Any failure here is fatal Ã¢â‚¬â€ we can't serve
    //    traffic without storage or telephony.
    let state = match AppState::bootstrap(config.clone()).await {
        Ok(s) => Arc::new(s),
        Err(e) => {
            error!(error = %e, "failed to bootstrap AppState");
            std::process::exit(1);
        }
    };

    // 5. Cancellation token drives graceful shutdown of background tasks.
    let shutdown = CancellationToken::new();
    state.spawn_background_tasks(shutdown.clone());

    // 6. Build and run the HTTP server.
    let bind_addr = format!("{}:{}", config.server.host, config.server.port);

    let state_for_server = state.clone();
    let shutdown_for_server = shutdown.clone();

    let server = HttpServer::new(move || {
        App::new()
            .app_data(web::Data::new(state_for_server.clone()))
            // JSON payload limit Ã¢â‚¬â€ transcripts are small but bound them anyway.
            .app_data(web::JsonConfig::default().limit(256 * 1024))
            // Path extractor max length (SessionId is a UUID-ish string).
            .app_data(web::PathConfig::default())
            .wrap(TracingLogger::default())
            .wrap(Compress::default())
            .wrap(middleware::RequestId::default())
            .route("/dashboard", web::get().to(handlers::dashboard::index))
            .route(fallback::AUDIO_PATH, web::get().to(fallback::audio))
            .service(
                web::scope("/api/v1")
                    .route("/health", web::get().to(handlers::health::health))
                    .route("/ready", web::get().to(handlers::health::ready))
                    .route("/metrics", web::get().to(handlers::health::metrics))
                    .route("/queue/raft", web::get().to(handlers::raft::status))
                    .route(
                        "/queue/raft/cluster",
                        web::get().to(handlers::raft::cluster),
                    )
                    .route("/calls/incoming", web::post().to(handlers::calls::incoming))
                    .route("/calls/{sid}", web::get().to(handlers::calls::get_session))
                    .route("/calls/{sid}/turn", web::post().to(handlers::calls::turn))
                    .route(
                        "/calls/{sid}/hangup",
                        web::post().to(handlers::calls::hangup),
                    )
                    .route(
                        "/calls/{sid}/acknowledge",
                        web::post().to(handlers::calls::acknowledge_operator),
                    )
                    .route(
                        "/volunteers",
                        web::get().to(handlers::volunteers::list_volunteers),
                    )
                    .route(
                        "/volunteers/queue",
                        web::get().to(handlers::volunteers::list_pending),
                    )
                    .route(
                        "/volunteers/{volunteer_id}/cases",
                        web::get().to(handlers::volunteers::list_assigned),
                    )
                    .route(
                        "/volunteers/{volunteer_id}/cases/{session_id}/resolve",
                        web::post().to(handlers::volunteers::resolve_case),
                    )
                    .route(
                        "/volunteers/{volunteer_id}/cases/{session_id}/transfer",
                        web::post().to(handlers::volunteers::transfer_case),
                    )
                    .route(
                        "/volunteers/claims/alerts",
                        web::get().to(handlers::volunteers::list_alerts),
                    )
                    .route(
                        "/volunteers/claim",
                        web::post().to(handlers::volunteers::claim),
                    )
                    .route(
                        "/operator/alerts",
                        web::get().to(handlers::volunteers::list_alerts),
                    ),
            )
            // Raft peer-to-peer traffic (token-protected; snapshots can exceed the default limit).
            .service(
                web::scope("/internal/raft")
                    .app_data(web::JsonConfig::default().limit(16 * 1024 * 1024))
                    .route("/append", web::post().to(handlers::raft::append))
                    .route("/vote", web::post().to(handlers::raft::vote))
                    .route("/snapshot", web::post().to(handlers::raft::snapshot))
                    .route("/write", web::post().to(handlers::raft::write)),
            )
            // Twilio-facing webhook scope (returns TwiML, not JSON).
            .service(
                web::scope("/twilio")
                    .route("/voice", web::post().to(handlers::twilio::voice))
                    .route("/gather", web::post().to(handlers::twilio::gather))
                    .route("/status", web::post().to(handlers::twilio::status_callback))
                    .route(
                        "/outbound-connect",
                        web::post().to(handlers::twilio::outbound_connect),
                    ),
            )
    })
    .workers(config.server.workers)
    .disable_signals()
    .shutdown_timeout(20) // Leaves room for the 5-second preStop within the 30-second grace.
    .bind(&bind_addr)?;

    info!(%bind_addr, "HTTP server bound");
    let display_host = match config.server.host.as_str() {
        "0.0.0.0" | "::" => "localhost",
        host => host,
    };
    info!(
        "Dashboard: http://{display_host}:{}/dashboard",
        config.server.port
    );

    serve_until_shutdown(server.run(), shutdown, shutdown_signal()).await?;

    info!("SayangCare shut down cleanly");
    Ok(())
}

async fn serve_until_shutdown(
    mut server: actix_web::dev::Server,
    shutdown: CancellationToken,
    signal: impl std::future::Future<Output = ()>,
) -> std::io::Result<()> {
    let control = server.handle();
    tokio::select! {
        result = &mut server => {
            shutdown.cancel();
            result
        }
        _ = signal => {
            info!("shutdown signal received; draining connections");
            shutdown.cancel();
            let (_, result) = tokio::join!(control.stop(true), server);
            result
        }
    }
}

/// Resolves when SIGINT (Ctrl-C) or SIGTERM (k8s pod deletion) arrives.
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl-C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => warn!("SIGINT received"),
        _ = terminate => warn!("SIGTERM received"),
    }
}

#[cfg(test)]
mod shutdown_tests {
    use super::*;
    use actix_web::HttpResponse;
    use std::net::TcpListener;
    use std::time::Duration;
    use tokio::sync::{oneshot, Notify};

    #[actix_web::test]
    async fn shutdown_waits_for_an_in_flight_response() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let entered = Arc::new(Notify::new());
        let notify = entered.clone();
        let server = HttpServer::new(move || {
            let notify = notify.clone();
            App::new().route(
                "/",
                web::get().to(move || {
                    let notify = notify.clone();
                    async move {
                        notify.notify_one();
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        HttpResponse::Ok().body("finished")
                    }
                }),
            )
        })
        .workers(1)
        .disable_signals()
        .shutdown_timeout(2)
        .listen(listener)
        .unwrap()
        .run();
        let shutdown = CancellationToken::new();
        let token = shutdown.clone();
        let (send, receive) = oneshot::channel();
        let serving = actix_web::rt::spawn(serve_until_shutdown(server, token, async {
            receive.await.unwrap();
        }));
        let request = actix_web::rt::spawn(async move {
            reqwest::Client::builder()
                .no_proxy()
                .build()
                .unwrap()
                .get(format!("http://{address}/"))
                .send()
                .await
                .unwrap()
                .text()
                .await
                .unwrap()
        });
        tokio::time::timeout(Duration::from_secs(3), entered.notified())
            .await
            .unwrap();
        send.send(()).unwrap();
        assert_eq!(request.await.unwrap(), "finished");
        tokio::time::timeout(Duration::from_secs(10), serving)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(shutdown.is_cancelled());
    }
}
