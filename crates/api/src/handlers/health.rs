use crate::state::AppState;
use actix_web::{web, HttpResponse, Responder};
use serde::Serialize;
use std::sync::Arc;
use std::time::Duration;

#[derive(Serialize)]
struct Health {
    status: &'static str,
    service: &'static str,
    version: &'static str,
}

/// Liveness probe — must be trivial; if this fails, k8s restarts the pod.
pub async fn health() -> impl Responder {
    HttpResponse::Ok().json(Health {
        status: "ok",
        service: "sayangcare",
        version: env!("CARGO_PKG_VERSION"),
    })
}

/// Readiness probe — don't route traffic when either backing store is unavailable.
pub async fn ready(state: web::Data<Arc<AppState>>) -> impl Responder {
    let (redis_result, postgres_result) = tokio::join!(
        tokio::time::timeout(Duration::from_secs(2), state.hot_sessions.ping()),
        tokio::time::timeout(
            Duration::from_secs(2),
            sqlx::query("SELECT 1").execute(&state.postgres),
        ),
    );
    let redis_ready = matches!(redis_result, Ok(Ok(())));
    let postgres_ready = matches!(postgres_result, Ok(Ok(_)));
    let ready = redis_ready && postgres_ready;

    if !redis_ready || !postgres_ready {
        tracing::warn!(redis_ready, postgres_ready, "readiness check failed");
    }

    let mut response = HttpResponse::build(readiness_status(redis_ready, postgres_ready));
    response.json(serde_json::json!({
        "ready": ready,
        "dependencies": {
            "redis": if redis_ready { "ok" } else { "unavailable" },
            "postgres": if postgres_ready { "ok" } else { "unavailable" },
        }
    }))
}

fn readiness_status(redis_ready: bool, postgres_ready: bool) -> actix_web::http::StatusCode {
    if redis_ready && postgres_ready {
        actix_web::http::StatusCode::OK
    } else {
        actix_web::http::StatusCode::SERVICE_UNAVAILABLE
    }
}

/// Prometheus scrape endpoint.
/// Wire `prometheus` or `metrics-exporter-prometheus` here later.
pub async fn metrics(state: web::Data<Arc<AppState>>) -> impl Responder {
    crate::perf_metrics::metrics()
        .pg_size
        .set(state.postgres.size() as i64);
    crate::perf_metrics::metrics()
        .pg_idle
        .set(state.postgres.num_idle() as i64);
    HttpResponse::Ok()
        .insert_header(("content-type", "text/plain; version=0.0.4"))
        .body(crate::telemetry::prometheus_metrics())
}

/// Utility for handlers needing typed JSON responses.
pub fn json_ok<T: Serialize>(value: T) -> HttpResponse {
    HttpResponse::Ok().json(value)
}

#[allow(dead_code)]
pub fn internal_error<E: std::fmt::Display>(e: E) -> HttpResponse {
    tracing::error!(error = %e, "internal error");
    HttpResponse::InternalServerError().json(serde_json::json!({
        "error": "internal",
    }))
}

pub fn _use_web() -> web::Json<()> {
    unreachable!()
}

#[cfg(test)]
mod tests {
    use super::readiness_status;
    use actix_web::http::StatusCode;

    #[test]
    fn readiness_requires_both_backing_stores() {
        assert_eq!(readiness_status(true, true), StatusCode::OK);
        assert_eq!(
            readiness_status(false, true),
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(
            readiness_status(true, false),
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(
            readiness_status(false, false),
            StatusCode::SERVICE_UNAVAILABLE
        );
    }
}
