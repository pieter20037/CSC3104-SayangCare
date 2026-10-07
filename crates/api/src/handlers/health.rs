use actix_web::{web, HttpResponse, Responder};
use serde::Serialize;

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

/// Readiness probe — could be extended to check Redis/Postgres reachability.
/// For now, mirrors liveness so k8s can route traffic immediately.
pub async fn ready() -> impl Responder {
    // TODO: ping Redis + Postgres and return 503 if either is down.
    HttpResponse::Ok().json(serde_json::json!({ "ready": true }))
}

/// Prometheus scrape endpoint.
/// Wire `prometheus` or `metrics-exporter-prometheus` here later.
pub async fn metrics() -> impl Responder {
    // Placeholder so HPA + Prometheus annotations don't 404.
    let body = "# SayangCare metrics placeholder\n";
    HttpResponse::Ok()
        .insert_header(("content-type", "text/plain; version=0.0.4"))
        .body(body)
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
