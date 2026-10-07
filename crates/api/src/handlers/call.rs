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
    let mut session = Session::with_id(SessionId(body.call_sid.clone()), caller);
    session.is_simulated = true;

    match state.sessions.create_if_absent(&session).await {
        Ok(true) => {
            info!(session_id = %session.id, "call accepted");
            HttpResponse::Ok().json(IncomingResponse {
                session_id: session.id.0,
                state: format!("{:?}", session.state).to_lowercase(),
            })
        }
        Ok(false) => match state.sessions.get(&session.id).await {
            Ok(Some(mut existing)) if existing.caller.phone_number == body.from => {
                if !existing.is_simulated {
                    let expected = existing.version;
                    existing.mark_simulated();
                    if let Err(error) = state.sessions.update_cas(&existing, expected).await {
                        tracing::warn!(error = %error, session_id = %existing.id, "failed to mark replayed simulator session");
                        return HttpResponse::Conflict().finish();
                    }
                }
                HttpResponse::Ok().json(IncomingResponse {
                    session_id: existing.id.0,
                    state: format!("{:?}", existing.state).to_lowercase(),
                })
            }
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
    let expected = session.version;
    let transcript = body.transcript.trim();

    if transcript.is_empty() {
        return HttpResponse::BadRequest().finish();
    }

    let assessment = sayangcare_core::domain::RiskAssessment::from_text(transcript);
    session.risk = assessment.clone();

    if session
        .record_turns([Turn {
            speaker: sayangcare_core::domain::Speaker::Caller,
            text: transcript.to_string(),
            timestamp: chrono::Utc::now(),
        }])
        .is_err()
    {
        return HttpResponse::Conflict().finish();
    }

    if session.risk.level.requires_immediate_escalation() {
        let is_repeat_escalation = session.state == sayangcare_core::domain::SessionState::Escalated
            || session.escalation_count > 0;

        if is_repeat_escalation {
            session.record_repeated_risk();
            state.record_alert(
                "repeated_risk",
                &session.id.0,
                None,
                format!(
                    "Repeated high-risk language on session {} increased the escalation counter to {} and requires operator acknowledgment.",
                    session.id.0,
                    session.escalation_count
                ),
            );
        } else if let Err(e) = session.transition(sayangcare_core::domain::SessionState::Escalated) {
            tracing::warn!(error = %e, session_id = %session.id, "failed to escalate session");
            return HttpResponse::Conflict().finish();
        }

        let queue_priority = session.risk.level.0.min(5);
        if let Err(e) = state
            .queue
            .enqueue(session.id.clone(), queue_priority, chrono::Utc::now())
            .await
        {
            tracing::warn!(error = %e, session_id = %session.id, "failed to queue escalated session");
        }

        if !is_repeat_escalation {
            state.record_alert(
                "escalated",
                &session.id.0,
                None,
                format!(
                    "Escalation alert raised for session {}. Risk level {} flagged for human review.",
                    session.id.0,
                    session.risk.level.0
                ),
            );
        }
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
        "state": format!("{:?}", session.state).to_lowercase(),
        "version": session.version,
        "risk_level": session.risk.level.0,
        "risk_rationale": session.risk.rationale,
    }))
}

pub async fn acknowledge_operator(
    state: web::Data<Arc<AppState>>,
    path: web::Path<String>,
) -> impl Responder {
    let sid = sayangcare_core::domain::SessionId(path.into_inner());

    let mut session = match state.sessions.get(&sid).await {
        Ok(Some(s)) => s,
        Ok(None) => return HttpResponse::NotFound().finish(),
        Err(e) => {
            tracing::error!(error = %e, "failed to load session for acknowledgment");
            return HttpResponse::InternalServerError().finish();
        }
    };

    let expected = session.version;
    session.acknowledge_operator();

    if let Err(e) = state.sessions.update_cas(&session, expected).await {
        tracing::warn!(error = %e, session_id = %sid, "failed to persist operator acknowledgment");
        return HttpResponse::Conflict().finish();
    }

    state.record_alert(
        "acknowledged",
        &sid.0,
        session.assigned_volunteer_id.as_deref(),
        format!("Operator acknowledged session {} after escalation review.", sid.0),
    );

    HttpResponse::Ok().json(serde_json::json!({
        "session_id": sid.0,
        "operator_acknowledged": session.operator_acknowledged,
        "escalation_count": session.escalation_count,
    }))
}

pub async fn hangup(state: web::Data<Arc<AppState>>, path: web::Path<String>) -> impl Responder {
    let sid = sayangcare_core::domain::SessionId(path.into_inner());

    let terminal_state = match state.sessions.get(&sid).await {
        Ok(Some(session))
            if matches!(
                session.state,
                sayangcare_core::domain::SessionState::Escalated
            ) =>
        {
            sayangcare_core::domain::SessionState::Escalated
        }
        _ => sayangcare_core::domain::SessionState::Completed,
    };

    match state.tiered.finish(&sid, terminal_state).await {
        Ok(true) => HttpResponse::Ok().json(serde_json::json!({
            "session_id": sid.0,
            "state": format!("{:?}", terminal_state).to_lowercase(),
        })),
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
