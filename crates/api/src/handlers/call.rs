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
        Ok(Some(session)) => {
            let sentiment_distress_severity = session
                .latest_sentiment
                .as_ref()
                .map(|sentiment| sentiment.distress_severity());
            let mut payload = match serde_json::to_value(&session) {
                Ok(payload) => payload,
                Err(error) => {
                    tracing::error!(error = %error, "failed to serialize session response");
                    return HttpResponse::InternalServerError().finish();
                }
            };
            if let Some(payload) = payload.as_object_mut() {
                payload.insert(
                    "sentiment_distress_severity".to_string(),
                    serde_json::json!(sentiment_distress_severity),
                );
            }
            HttpResponse::Ok().json(payload)
        }
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

fn response_for_risk(level: u8) -> &'static str {
    match level {
        4..=u8::MAX => "Thank you for telling me. Don't worry, I've alerted a human volunteer. They'll speak with you as soon as one is available. Please stay with me while we connect you.",
        2..=3 => "I'm glad you told me. That sounds difficult to carry. What feels most important for me to understand right now?",
        _ => "Thank you for sharing that with me. What has been weighing on you most today?",
    }
}

pub async fn turn(
    state: web::Data<Arc<AppState>>,
    path: web::Path<String>,
    body: web::Json<TurnRequest>,
) -> impl Responder {
    let budget = crate::performance::TurnBudget::new("json", &state.config.performance);
    let sid = sayangcare_core::domain::SessionId(path.into_inner());

    let mut session = match budget
        .storage("storage_read", state.sessions.get(&sid))
        .await
    {
        Ok(Some(s)) => s,
        Ok(None) => return HttpResponse::NotFound().finish(),
        Err(e) => {
            tracing::error!(error = %e, "store error");
            return HttpResponse::InternalServerError().finish();
        }
    };

    let expected = session.version;
    let transcript = body.transcript.trim();

    if transcript.is_empty() {
        return HttpResponse::BadRequest().finish();
    }

    let prior_assessment = session
        .transcript
        .turns
        .iter()
        .filter(|turn| turn.speaker == sayangcare_core::domain::Speaker::Caller)
        .map(|turn| sayangcare_core::domain::RiskAssessment::from_text(&turn.text))
        .max_by_key(|assessment| assessment.level);
    let current_assessment = sayangcare_core::domain::RiskAssessment::from_text(transcript);
    let current_turn_is_high_risk = current_assessment.level.requires_immediate_escalation();
    let assessment = prior_assessment
        .filter(|prior| prior.level > current_assessment.level)
        .unwrap_or(current_assessment);
    // Once a session is escalated, a later harmless utterance ("hi", silence,
    // etc.) must not erase the risk that triggered the handoff.
    session.risk = if session.state == sayangcare_core::domain::SessionState::Escalated
        && assessment.level < session.risk.level
    {
        // Keep the highest assessment seen during the active handoff.
        session.risk.clone()
    } else {
        assessment
    };
    let was_escalated = session.state == sayangcare_core::domain::SessionState::Escalated;
    let escalated = was_escalated || session.risk.level.requires_immediate_escalation();

    let now = chrono::Utc::now();
    let caller_turn = Turn {
        speaker: sayangcare_core::domain::Speaker::Caller,
        text: transcript.to_string(),
        timestamp: now,
    };
    let mut use_audio = false;
    let assistant_response = if escalated {
        response_for_risk(session.risk.level.0).to_string()
    } else if state.inference.is_some() {
        match crate::sentiment::reply_for_turn(&state, &budget, &mut session, &caller_turn).await {
            Ok(reply) => reply,
            Err(error) => {
                tracing::warn!(%error, session_id = %session.id, "AI unavailable; using bounded audio fallback");
                use_audio = true;
                crate::fallback::HOLDING_MESSAGE.to_string()
            }
        }
    } else {
        response_for_risk(session.risk.level.0).to_string()
    };

    if session
        .record_turns([
            caller_turn,
            Turn {
                speaker: sayangcare_core::domain::Speaker::Assistant,
                text: assistant_response.clone(),
                timestamp: chrono::Utc::now(),
            },
        ])
        .is_err()
    {
        return HttpResponse::Conflict().finish();
    }

    if crate::fallback::mark_degraded(&mut session, use_audio).is_err() {
        return HttpResponse::Conflict().finish();
    }

    if escalated {
        if was_escalated {
            if current_turn_is_high_risk {
                session.record_repeated_risk();
                let alert = state.try_record_alert(
                    "repeated_risk",
                    &session.id.0,
                    None,
                    format!(
                        "Repeated high-risk language on session {} increased the escalation counter to {} and requires operator acknowledgment.",
                        session.id.0,
                        session.escalation_count
                    ),
                );
                if let Err(error) = budget.auxiliary("alerts", alert).await {
                    tracing::error!(%error, session_id = %session.id, "bounded operator alert write failed");
                }
            }
        } else {
            if let Err(e) = session.transition(sayangcare_core::domain::SessionState::Escalated) {
                tracing::warn!(error = %e, session_id = %session.id, "failed to escalate session");
                return HttpResponse::Conflict().finish();
            }
            let alert = state.try_record_alert(
                "escalated",
                &session.id.0,
                None,
                format!(
                    "Escalation alert raised for session {}. Risk level {} flagged for human review.",
                    session.id.0,
                    session.risk.level.0
                ),
            );
            if let Err(error) = budget.auxiliary("alerts", alert).await {
                tracing::error!(%error, session_id = %session.id, "bounded operator alert write failed");
            }
        }

        if !was_escalated || current_turn_is_high_risk {
            let queue_priority = session.risk.level.0.min(5);
            if let Err(e) = budget
                .auxiliary(
                    "queue",
                    state
                        .queue
                        .enqueue(session.id.clone(), queue_priority, chrono::Utc::now()),
                )
                .await
            {
                tracing::warn!(error = %e, session_id = %session.id, "failed to queue escalated session");
            }
        }
    }

    if let Err(e) = budget
        .storage(
            "storage_write",
            state.sessions.update_cas(&session, expected),
        )
        .await
    {
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
        "assistant_response": assistant_response,
        "fallback_audio_url": use_audio.then(|| crate::fallback::audio_url(&state.config.telephony.public_base_url)),
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

    state
        .record_alert(
            "acknowledged",
            &sid.0,
            session.assigned_volunteer_id.as_deref(),
            format!(
                "Operator acknowledged session {} after escalation review.",
                sid.0
            ),
        )
        .await;

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
