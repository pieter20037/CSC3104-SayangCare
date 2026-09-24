use crate::state::AppState;
use actix_web::{web, HttpResponse, Responder};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Deserialize)]
pub struct ClaimRequest {
    pub volunteer_id: String,
}

#[derive(Debug, Serialize)]
pub struct ClaimResponse {
    pub session_id: Option<String>,
}

pub async fn claim(
    state: web::Data<Arc<AppState>>,
    body: web::Json<ClaimRequest>,
) -> impl Responder {
    match state.queue.claim(&body.volunteer_id).await {
        Ok(Some(sid)) => HttpResponse::Ok().json(ClaimResponse {
            session_id: Some(sid.0),
        }),
        Ok(None) => HttpResponse::Ok().json(ClaimResponse { session_id: None }),
        Err(e) => {
            tracing::error!(error = %e, "claim failed");
            HttpResponse::InternalServerError().finish()
        }
    }
}