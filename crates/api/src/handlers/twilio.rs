//! Twilio-facing webhooks. These return TwiML (XML), not JSON.
//!
//! Twilio flow for SayangCare:
//!   1. Caller dials the number.
//!   2. Twilio POSTs to /twilio/voice.
//!   3. We respond with <Gather> to capture speech.
//!   4. Twilio POSTs the transcript to /twilio/gather.
//!   5. We run it through the LLM + circuit breaker and respond with <Say> or <Play>.
//!   6. On hangup, Twilio POSTs to /twilio/status.

use crate::middleware::twilio_signature::verify_twilio_signature;
use crate::state::AppState;
use actix_web::{http::header, web, HttpRequest, HttpResponse, Responder};
use sayangcare_core::domain::{CallerId, Session, SessionId, Speaker, Turn};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tracing::{info, warn};

const GATHER_ACTION: &str = "/twilio/gather";
const STATUS_CALLBACK: &str = "/twilio/status";

/// Entry point for an incoming call.
pub async fn voice(
    state: web::Data<Arc<AppState>>,
    request: HttpRequest,
    form: web::Form<HashMap<String, String>>,
) -> impl Responder {
    if !valid_twilio_request(&request, &form, &state) {
        warn!("rejected Twilio voice webhook with invalid signature");
        return HttpResponse::Forbidden().finish();
    }
    let (Some(call_sid), Some(from), Some(_to)) =
        (form.get("CallSid"), form.get("From"), form.get("To"))
    else {
        return HttpResponse::BadRequest().finish();
    };
    info!(call_sid = %call_sid, from = %from, "incoming call");

    // Twilio retries a webhook with the same CallSid; Redis atomically preserves its first state.
    let session = Session::with_id(
        SessionId(call_sid.clone()),
        CallerId {
            phone_number: from.clone(),
            display_name: None,
        },
    );

    match state.sessions.create_if_absent(&session).await {
        Ok(true) => {}
        Ok(false) => match state.sessions.get(&session.id).await {
            Ok(Some(existing)) if existing.caller.phone_number == *from => {}
            Ok(_) => return twiml_error("We couldn't match this call. Please call back."),
            Err(e) => {
                warn!(error = %e, "failed to load existing session");
                return twiml_error("We're having trouble connecting you. Please try again.");
            }
        },
        Err(e) => {
            warn!(error = %e, "failed to persist new session");
            return twiml_error("We're having trouble connecting you. Please try again.");
        }
    }

    let public_base_url = state.config.telephony.public_base_url.trim_end_matches('/');
    let action = format!("{public_base_url}{GATHER_ACTION}");
    let status_cb = format!("{public_base_url}{STATUS_CALLBACK}");

    if let Err(error) = state
        .telephony
        .register_status_callback(call_sid, &status_cb)
        .await
    {
        warn!(error = %error, call_sid = %call_sid, "failed to register Twilio status callback");
    }

    // Greet + gather speech.
    let body = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<Response>
  <Say voice="Polly.Joanna">Hello, and welcome to SayangCare. I'm here to listen. How are you feeling today?</Say>
  <Gather input="speech" speechTimeout="auto" action="{action}" method="POST" actionOnEmptyResult="true">
    <Say voice="Polly.Joanna">Please go ahead, I'm listening.</Say>
  </Gather>
  <Redirect method="POST">{action}</Redirect>
</Response>"#
    );

    xml_response(body)
}

/// Handles the transcribed speech from a <Gather>.
pub async fn gather(
    state: web::Data<Arc<AppState>>,
    request: HttpRequest,
    form: web::Form<HashMap<String, String>>,
) -> impl Responder {
    if !valid_twilio_request(&request, &form, &state) {
        warn!("rejected Twilio gather webhook with invalid signature");
        return HttpResponse::Forbidden().finish();
    }
    let Some(call_sid) = form.get("CallSid") else {
        return HttpResponse::BadRequest().finish();
    };
    let sid = SessionId(call_sid.clone());
    let speech = form
        .get("SpeechResult")
        .cloned()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "(silence)".to_string());

    // Load current session.
    let mut session = match state.sessions.get(&sid).await {
        Ok(Some(s)) => s,
        Ok(None) => {
            warn!(call_sid = %call_sid, "session missing on gather");
            return twiml_error("Your session expired. Please call back.");
        }
        Err(e) => {
            warn!(error = %e, "store error on gather");
            return twiml_error("Technical difficulty. Please call back.");
        }
    };

    let expected = session.version;
    let prior_assessment = session
        .transcript
        .turns
        .iter()
        .filter(|turn| turn.speaker == Speaker::Caller)
        .map(|turn| sayangcare_core::domain::RiskAssessment::from_text(&turn.text))
        .max_by_key(|assessment| assessment.level);
    let current_assessment = sayangcare_core::domain::RiskAssessment::from_text(&speech);
    let current_turn_is_high_risk = current_assessment
        .level
        .requires_immediate_escalation();
    let assessment = prior_assessment
        .filter(|prior| prior.level > current_assessment.level)
        .unwrap_or(current_assessment);
    // Keep the highest observed risk throughout an active human handoff. A
    // benign follow-up must not drop the caller back into ordinary LLM chat.
    session.risk = if session.state == sayangcare_core::domain::SessionState::Escalated
        && assessment.level < session.risk.level
    {
        session.risk.clone()
    } else {
        assessment
    };
    let caller_turn = Turn {
        speaker: Speaker::Caller,
        text: speech.clone(),
        timestamp: chrono::Utc::now(),
    };

    let was_escalated = session.state == sayangcare_core::domain::SessionState::Escalated;
    let escalated = was_escalated || session.risk.level.requires_immediate_escalation();
    let assistant_reply = if escalated {
        "Thank you for telling me. I'm sorry you're going through this. I want to help keep you safe, and I'm alerting a human volunteer now. Are you in immediate danger right now?".to_string()
    } else if let Some(inference) = state.inference.as_ref() {
        let breaker = state.breakers.get("llm-inference").await;
        match breaker.acquire_with_permit() {
            Err(_) => {
                // Circuit is open, or another recovery probe is running.
                "I'm here with you. Please tell me a little more about what is happening."
                    .to_string()
            }
            Ok(permit) => {
                let mut context = session.transcript.clone();
                context.push(caller_turn.clone());
                let started = Instant::now();

                match inference.generate_reply(&context).await {
                    Ok(reply) => {
                        let latency = started.elapsed().as_millis() as u64;
                        breaker.record_success_with_permit(permit, latency);
                        reply
                    }
                    Err(error) => {
                        let latency = started.elapsed().as_millis() as u64;
                        breaker
                            .record_failure_with_permit(permit, latency, 0.25)
                            .await;
                        warn!(
                            error = %error,
                            call_sid = %call_sid,
                            "Groq reply failed; using deterministic fallback"
                        );
                        "I'm here with you. Please tell me a little more about what is happening."
                            .to_string()
                    }
                }
            }
        }
    } else {
        "I'm here with you. Please tell me a little more about what is happening.".to_string()
    };

    if session
        .record_turns([
            caller_turn,
            Turn {
                speaker: Speaker::Assistant,
                text: assistant_reply.clone(),
                timestamp: chrono::Utc::now(),
            },
        ])
        .is_err()
    {
        return twiml_error("Your session cannot continue in its current state.");
    }

    if escalated {
        if was_escalated {
            if current_turn_is_high_risk {
                session.record_repeated_risk();
                state.record_alert(
                    "repeated_risk",
                    &session.id.0,
                    None,
                    format!(
                        "Repeated high-risk language on live call {} requires operator review.",
                        session.id.0
                    ),
                );
            }
        } else {
            if let Err(error) = session.transition(sayangcare_core::domain::SessionState::Escalated) {
                warn!(error = %error, call_sid = %call_sid, "failed to transition high-risk call to escalated");
                return twiml_error("I want to connect you with a human supporter, but our system is having trouble. Please contact local emergency services or someone you trust now.");
            }
            state.record_alert(
                "escalated",
                &session.id.0,
                None,
                format!(
                    "Live call {} escalated at risk level {}.",
                    session.id.0, session.risk.level.0
                ),
            );
        }

        if !was_escalated || current_turn_is_high_risk {
            if let Err(error) = state
                .queue
                .enqueue(session.id.clone(), session.risk.level.0, chrono::Utc::now())
                .await
            {
                warn!(error = %error, call_sid = %call_sid, "failed to enqueue high-risk live call");
            }
        }
    }
    if let Err(e) = state.sessions.update_cas(&session, expected).await {
        warn!(error = %e, "CAS failed on gather — retrying next turn");
        return twiml_error("Your session changed concurrently. Please try again.");
    }

    // Respond with the assistant's spoken reply + gather again.
    let action = format!(
        "{}{GATHER_ACTION}",
        state.config.telephony.public_base_url.trim_end_matches('/')
    );
    let escaped = xml_escape(&assistant_reply);
    let body = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<Response>
  <Say voice="Polly.Joanna">{escaped}</Say>
  <Gather input="speech" speechTimeout="auto" action="{action}" method="POST" actionOnEmptyResult="true">
    <Say voice="Polly.Joanna">I'm still here.</Say>
  </Gather>
  <Redirect method="POST">{action}</Redirect>
</Response>"#
    );
    xml_response(body)
}

/// Twilio status callback — fires when the call ends.
/// Twilio requests these instructions when the dispatched volunteer answers.
pub async fn outbound_connect(
    state: web::Data<Arc<AppState>>,
    request: HttpRequest,
    query: web::Query<HashMap<String, String>>,
    form: web::Form<HashMap<String, String>>,
) -> impl Responder {
    if !valid_twilio_request(&request, &form, &state) {
        warn!("rejected Twilio outbound-connect webhook with invalid signature");
        return HttpResponse::Forbidden().finish();
    }
    let (Some(session_id), Some(call_sid)) = (query.get("session_id"), form.get("CallSid")) else {
        return HttpResponse::BadRequest().finish();
    };
    let sid = SessionId(session_id.clone());
    let session = match state.sessions.get(&sid).await {
        Ok(Some(session)) => session,
        Ok(None) => return twiml_error("This support call is no longer available."),
        Err(error) => {
            warn!(error = %error, session_id = %sid, call_sid = %call_sid, "failed to load outbound call session");
            return twiml_error("We're having trouble connecting you. Please try again later.");
        }
    };
    if session.assigned_volunteer_id.is_none() {
        return twiml_error("This support request is no longer assigned.");
    }

    let room = xml_escape(&conference_name(&sid.0));
    let body = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<Response>
  <Say voice="Polly.Joanna">Thank you for supporting this caller. Connecting you now.</Say>
  <Dial><Conference startConferenceOnEnter="true" endConferenceOnExit="true" beep="false">{room}</Conference></Dial>
</Response>"#
    );
    xml_response(body)
}

pub fn conference_name(session_id: &str) -> String {
    let encoded_id = session_id
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("sayangcare-{encoded_id}")
}

pub async fn status_callback(
    state: web::Data<Arc<AppState>>,
    request: HttpRequest,
    form: web::Form<HashMap<String, String>>,
) -> impl Responder {
    if !valid_twilio_request(&request, &form, &state) {
        warn!("rejected Twilio status webhook with invalid signature");
        return HttpResponse::Forbidden().finish();
    }
    let (Some(call_sid), Some(call_status)) = (form.get("CallSid"), form.get("CallStatus")) else {
        return HttpResponse::BadRequest().finish();
    };
    if matches!(
        call_status.as_str(),
        "completed" | "failed" | "busy" | "no-answer" | "canceled"
    ) {
        let sid = SessionId(call_sid.clone());
        let terminal_state = if call_status != "completed" {
            sayangcare_core::domain::SessionState::Failed
        } else {
            sayangcare_core::domain::SessionState::Completed
        };
        match state.tiered.finish(&sid, terminal_state).await {
            Ok(true) => info!(call_sid = %call_sid, "session archived"),
            Ok(false) => warn!(call_sid = %call_sid, "session not found on status callback"),
            Err(e) => {
                warn!(error = %e, call_sid = %call_sid, "cold flush failed");
                return HttpResponse::InternalServerError().finish();
            }
        }
    }
    HttpResponse::Ok().finish()
}

fn valid_twilio_request(
    request: &HttpRequest,
    form: &HashMap<String, String>,
    state: &AppState,
) -> bool {
    verify_twilio_signature(
        request,
        form,
        &state.config.telephony.public_base_url,
        &state.config.telephony.twilio_auth_token,
    )
}

fn xml_response(body: String) -> HttpResponse {
    HttpResponse::Ok()
        .insert_header((header::CONTENT_TYPE, "text/xml; charset=utf-8"))
        .body(body)
}

fn twiml_error(msg: &str) -> HttpResponse {
    let body = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<Response><Say voice="Polly.Joanna">{}</Say><Hangup/></Response>"#,
        xml_escape(msg)
    );
    xml_response(body)
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
