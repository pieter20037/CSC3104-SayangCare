//! Twilio-facing webhooks. These return TwiML (XML), not JSON.
//!
//! Twilio flow for SayangCare:
//!   1. Caller dials the number.
//!   2. Twilio POSTs to /twilio/voice.
//!   3. We respond with <Gather> to capture speech.
//!   4. Twilio POSTs the transcript to /twilio/gather.
//!   5. We run it through the LLM + circuit breaker and respond with <Say> or <Play>.
//!   6. On hangup, Twilio POSTs to /twilio/status.

use crate::state::AppState;
use actix_web::{http::header, web, HttpResponse, Responder};
use sayangcare_core::domain::{CallerId, Session, SessionId, Speaker, Turn};
use serde::Deserialize;
use std::sync::Arc;
use std::time::Instant;
use tracing::{info, warn};

const GATHER_ACTION: &str = "/twilio/gather";
const HANGUP_ACTION: &str = "/twilio/status";

#[derive(Debug, Deserialize)]
pub struct VoiceWebhook {
    #[serde(rename = "CallSid")]
    pub call_sid: String,
    #[serde(rename = "From")]
    pub from: String,
    #[serde(rename = "To")]
    pub to: String,
}

/// Entry point for an incoming call.
pub async fn voice(
    state: web::Data<Arc<AppState>>,
    form: web::Form<VoiceWebhook>,
) -> impl Responder {
    info!(call_sid = %form.call_sid, from = %form.from, "incoming call");

    // Twilio retries a webhook with the same CallSid; Redis atomically preserves its first state.
    let session = Session::with_id(
        SessionId(form.call_sid.clone()),
        CallerId {
            phone_number: form.from.clone(),
            display_name: None,
        },
    );

    match state.sessions.create_if_absent(&session).await {
        Ok(true) => {}
        Ok(false) => match state.sessions.get(&session.id).await {
            Ok(Some(existing)) if existing.caller.phone_number == form.from => {}
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

    let action = format!(
        "{}{}",
        state.config.telephony.public_base_url, GATHER_ACTION
    );
    let status_cb = format!(
        "{}{}",
        state.config.telephony.public_base_url, HANGUP_ACTION
    );

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

    // Set Twilio status callback on the call itself (done out of band
    // via the Twilio REST API in a real deployment).
    let _ = status_cb;

    xml_response(body)
}

#[derive(Debug, Deserialize)]
pub struct GatherWebhook {
    #[serde(rename = "CallSid")]
    pub call_sid: String,
    #[serde(rename = "SpeechResult", default)]
    pub speech_result: Option<String>,
}

/// Handles the transcribed speech from a <Gather>.
pub async fn gather(
    state: web::Data<Arc<AppState>>,
    form: web::Form<GatherWebhook>,
) -> impl Responder {
    let sid = SessionId(form.call_sid.clone());
    let speech = form
        .speech_result
        .clone()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "(silence)".to_string());

    // Load current session.
    let mut session = match state.sessions.get(&sid).await {
        Ok(Some(s)) => s,
        Ok(None) => {
            warn!(call_sid = %form.call_sid, "session missing on gather");
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

    let escalated = session.state == sayangcare_core::domain::SessionState::Escalated
        || session.risk.level.requires_immediate_escalation();
    let assistant_reply = if escalated {
        "Thank you for telling me. I'm sorry you're going through this. I want to help keep you safe, and I'm alerting a human volunteer now. Are you in immediate danger right now?".to_string()
    } else if let Some(inference) = state.inference.as_ref() {
        let breaker = state.breakers.get("llm-inference").await;
        if breaker.acquire().is_err() {
            "I'm here with you. Please tell me a little more about what is happening.".to_string()
        } else {
            let mut context = session.transcript.clone();
            context.push(caller_turn.clone());
            let started = Instant::now();
            match inference.generate_reply(&context).await {
                Ok(reply) => {
                    breaker.record_success(started.elapsed().as_millis() as u64);
                    reply
                }
                Err(error) => {
                    breaker
                        .record_failure(started.elapsed().as_millis() as u64, 0.25)
                        .await;
                    warn!(error = %error, call_sid = %form.call_sid, "Groq reply failed; using deterministic fallback");
                    "I'm here with you. Please tell me a little more about what is happening."
                        .to_string()
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
        let is_repeat_escalation = session.state
            == sayangcare_core::domain::SessionState::Escalated
            || session.escalation_count > 0;
        if is_repeat_escalation {
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
        } else if let Err(error) =
            session.transition(sayangcare_core::domain::SessionState::Escalated)
        {
            warn!(error = %error, call_sid = %form.call_sid, "failed to transition high-risk call to escalated");
            return twiml_error("I want to connect you with a human supporter, but our system is having trouble. Please contact local emergency services or someone you trust now.");
        }

        if let Err(error) = state
            .queue
            .enqueue(session.id.clone(), session.risk.level.0, chrono::Utc::now())
            .await
        {
            warn!(error = %error, call_sid = %form.call_sid, "failed to enqueue high-risk live call");
        }
        if !is_repeat_escalation {
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
    }
    if let Err(e) = state.sessions.update_cas(&session, expected).await {
        warn!(error = %e, "CAS failed on gather — retrying next turn");
        return twiml_error("Your session changed concurrently. Please try again.");
    }

    // Respond with the assistant's spoken reply + gather again.
    let action = format!(
        "{}{}",
        state.config.telephony.public_base_url, GATHER_ACTION
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

#[derive(Debug, Deserialize)]
pub struct StatusWebhook {
    #[serde(rename = "CallSid")]
    pub call_sid: String,
    #[serde(rename = "CallStatus")]
    pub call_status: String,
}

/// Twilio status callback — fires when the call ends.
pub async fn status_callback(
    state: web::Data<Arc<AppState>>,
    form: web::Form<StatusWebhook>,
) -> impl Responder {
    if form.call_status == "completed" || form.call_status == "failed" {
        let sid = SessionId(form.call_sid.clone());
        let terminal_state = if form.call_status == "failed" {
            sayangcare_core::domain::SessionState::Failed
        } else {
            sayangcare_core::domain::SessionState::Completed
        };
        match state.tiered.finish(&sid, terminal_state).await {
            Ok(true) => info!(call_sid = %form.call_sid, "session archived"),
            Ok(false) => warn!(call_sid = %form.call_sid, "session not found on status callback"),
            Err(e) => {
                warn!(error = %e, call_sid = %form.call_sid, "cold flush failed");
                return HttpResponse::InternalServerError().finish();
            }
        }
    }
    HttpResponse::Ok().finish()
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
