use crate::state::AppState;
use actix_web::{web, HttpResponse, Responder};
use sayangcare_core::domain::{CallerId, Session};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::info;

#[derive(Debug, Deserialize)]
pub struct IncomingCall {
    pub call_sid: String,
    pub from: String,
    pub to: String,
}

#[derive(Debug, Serialize)]
pub struct IncomingResponse {
    pub session_id: String,
    pub state: String,
}

pub async fn incoming(
    state: web::Data<Arc<AppState>>,
    body: web::Json<IncomingCall>,
) -> impl Responder {
    let caller = CallerId {
        phone_number: body.from.clone(),
        display_name: None,
    };
    let session = Session::new(caller);

    match state.sessions.put(&session).await {
        Ok(()) => {
            info!(session_id = %session.id, "call accepted");
            HttpResponse::Ok().json(IncomingResponse {
                session_id: session.id.0,
                state: format!("{:?}", session.state).to_lowercase(),
            })
        }
        Err(e) => {
            tracing::error!(error = %e, "failed to create session");
            HttpResponse::InternalServerError().finish()
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct TurnRequest {
    pub transcript: String,
}

pub async fn turn(
    state: web::Data<Arc<AppState>>,
    path: web::Path<String>,
    body: web::Json<TurnRequest>,
) -> impl Responder {
    let sid = sayangcare_core::domain::SessionId(path.into_inner());

    let mut session = match state.sessions.get(&sid).await {
        Ok(Some(s)) => s,
        Ok(None) => return HttpResponse::NotFound().finish(),
        Err(e) => {
            tracing::error!(error = %e, "store error");
            return HttpResponse::InternalServerError().finish();
        }
    };

    // Circuit-breaker-guarded LLM call would go here:
    //   let breaker = state.breakers.get("llm-inference").await;
    //   if breaker.acquire().is_err() { /* fallback to buffered audio */ }
    // For scaffold, we just record the turn.
    session.transcript.push(sayangcare_core::domain::Turn {
        speaker: sayangcare_core::domain::Speaker::Caller,
        text: body.transcript.clone(),
        timestamp: chrono::Utc::now(),
    });

    let expected = session.version;
    session.version += 1;
    session.updated_at = chrono::Utc::now();

    if let Err(e) = state.sessions.update_cas(&session, expected).await {
        tracing::warn!(error = %e, "CAS update failed");
        return HttpResponse::Conflict().finish();
    }

    HttpResponse::Ok().json(serde_json::json!({
        "session_id": session.id.0,
        "version": session.version,
    }))
}

pub async fn hangup(
    state: web::Data<Arc<AppState>>,
    path: web::Path<String>,
) -> impl Responder {
    let sid = sayangcare_core::domain::SessionId(path.into_inner());
    if let Ok(Some(mut session)) = state.sessions.get(&sid).await {
        let _ = session.transition(sayangcare_core::domain::SessionState::Completed);
        if let Err(e) = state.tiered.flush_to_cold(&session).await {
            tracing::error!(error = %e, "flush failed");
            return HttpResponse::InternalServerError().finish();
        }
    }
    HttpResponse::Ok().finish()
}