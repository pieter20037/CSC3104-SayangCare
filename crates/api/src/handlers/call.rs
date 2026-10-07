use crate::state::AppState;
use actix_web::{web, HttpResponse, Responder};
use sayangcare_core::domain::{CallerId, Session, SessionId, Turn};
use sayangcare_core::CoreError;
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

pub async fn get_session(
    state: web::Data<Arc<AppState>>,
    path: web::Path<String>,
) -> impl Responder {
    let sid = SessionId(path.into_inner());
    match state.sessions.get(&sid).await {
        Ok(Some(session)) => HttpResponse::Ok().json(session),
        Ok(None) => HttpResponse::NotFound().finish(),
        Err(e) => {
            tracing::error!(error = %e, "failed to load session");
            HttpResponse::InternalServerError().finish()
        }
    }
}

pub async fn incoming(
    state: web::Data<Arc<AppState>>,
    body: web::Json<IncomingCall>,
) -> impl Responder {
    let caller = CallerId {
        phone_number: body.from.clone(),
        display_name: None,
    };
    let session = Session::with_id(SessionId(body.call_sid.clone()), caller);

    match state.sessions.create_if_absent(&session).await {
        Ok(true) => {
            info!(session_id = %session.id, "call accepted");
            HttpResponse::Ok().json(IncomingResponse {
                session_id: session.id.0,
                state: format!("{:?}", session.state).to_lowercase(),
            })
        }
        Ok(false) => match state.sessions.get(&session.id).await {
            Ok(Some(existing)) if existing.caller.phone_number == body.from => HttpResponse::Ok()
                .json(IncomingResponse {
                    session_id: existing.id.0,
                    state: format!("{:?}", existing.state).to_lowercase(),
                }),
            Ok(_) => HttpResponse::Conflict().finish(),
            Err(e) => {
                tracing::error!(error = %e, "failed to load existing session");
                HttpResponse::InternalServerError().finish()
            }
        },
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
    let expected = session.version;
    if session
        .record_turns([Turn {
            speaker: sayangcare_core::domain::Speaker::Caller,
            text: body.transcript.clone(),
            timestamp: chrono::Utc::now(),
        }])
        .is_err()
    {
        return HttpResponse::Conflict().finish();
    }

    if let Err(e) = state.sessions.update_cas(&session, expected).await {
        tracing::warn!(error = %e, "CAS update failed");
        return match e {
            CoreError::VersionConflict { .. } => HttpResponse::Conflict().finish(),
            _ => HttpResponse::InternalServerError().finish(),
        };
    }

    HttpResponse::Ok().json(serde_json::json!({
        "session_id": session.id.0,
        "version": session.version,
    }))
}

pub async fn hangup(state: web::Data<Arc<AppState>>, path: web::Path<String>) -> impl Responder {
    let sid = sayangcare_core::domain::SessionId(path.into_inner());
    match state
        .tiered
        .finish(&sid, sayangcare_core::domain::SessionState::Completed)
        .await
    {
        Ok(true) => HttpResponse::Ok().finish(),
        Ok(false) => HttpResponse::NotFound().finish(),
        Err(sayangcare_core::CoreError::VersionConflict { .. }) => {
            HttpResponse::Conflict().finish()
        }
        Err(e) => {
            tracing::error!(error = %e, "failed to complete session");
            HttpResponse::InternalServerError().finish()
        }
    }
}
