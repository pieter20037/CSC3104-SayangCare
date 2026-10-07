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

mod handlers;
mod middleware;
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

    // 4. Bootstrap AppState. Any failure here is fatal — we can't serve
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
            // JSON payload limit — transcripts are small but bound them anyway.
            .app_data(web::JsonConfig::default().limit(256 * 1024))
            // Path extractor max length (SessionId is a UUID-ish string).
            .app_data(web::PathConfig::default())
            .wrap(TracingLogger::default())
            .wrap(Compress::default())
            .wrap(middleware::RequestId::default())
            .route("/dashboard", web::get().to(handlers::dashboard::index))
            .service(
                web::scope("/api/v1")
                    .route("/health", web::get().to(handlers::health::health))
                    .route("/ready", web::get().to(handlers::health::ready))
                    .route("/metrics", web::get().to(handlers::health::metrics))
                    .route("/calls/incoming", web::post().to(handlers::calls::incoming))
                    .route("/calls/{sid}", web::get().to(handlers::calls::get_session))
                    .route("/calls/{sid}/turn", web::post().to(handlers::calls::turn))
                    .route(
                        "/calls/{sid}/hangup",
                        web::post().to(handlers::calls::hangup),
                    )
                    .route(
                        "/volunteers/claim",
                        web::post().to(handlers::volunteers::claim),
                    ),
            )
            // Twilio-facing webhook scope (returns TwiML, not JSON).
            .service(
                web::scope("/twilio")
                    .route("/voice", web::post().to(handlers::twilio::voice))
                    .route("/gather", web::post().to(handlers::twilio::gather))
                    .route("/status", web::post().to(handlers::twilio::status_callback)),
            )
    })
    .workers(config.server.workers)
    .shutdown_timeout(25) // < terminationGracePeriodSeconds in k8s (30)
    .bind(&bind_addr)?;

    info!(%bind_addr, "HTTP server bound");

    let handle = server.run();

    // 7. Wait for either server exit or shutdown signal.
    let cancel_on_signal = async {
        shutdown_signal().await;
        info!("shutdown signal received; draining connections");
        shutdown.cancel();
    };

    tokio::select! {
        res = handle => {
            if let Err(e) = res {
                error!(error = %e, "server error");
                return Err(e.into());
            }
        }
        _ = cancel_on_signal => {
            // Graceful: give in-flight requests up to shutdown_timeout to finish.
            // (Actix handles this internally once run() returns.)
        }
    }

    info!("SayangCare shut down cleanly");
    Ok(())
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
