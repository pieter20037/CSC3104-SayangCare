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

    // Create session bound to the Twilio CallSid (so hangup can find it).
    let mut session = Session::new(CallerId {
        phone_number: form.from.clone(),
        display_name: None,
    });
    // Overwrite the internal id with CallSid so subsequent webhooks map cleanly.
    session.id = SessionId(form.call_sid.clone());

    if let Err(e) = state.sessions.put(&session).await {
        warn!(error = %e, "failed to persist new session");
        return twiml_error("We're having trouble connecting you. Please try again.");
    }

    let action = format!("{}{}", state.config.telephony.public_base_url, GATHER_ACTION);
    let status_cb = format!("{}{}", state.config.telephony.public_base_url, HANGUP_ACTION);

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

    // Record caller turn.
    session.transcript.push(Turn {
        speaker: Speaker::Caller,
        text: speech,
        timestamp: chrono::Utc::now(),
    });

    // --- Circuit-breaker-guarded LLM call ---
    let breaker = state.breakers.get("llm-inference").await;

    if breaker.acquire().is_err() {
        // OPEN: park the caller with buffered audio, escalate if high risk.
        warn!(call_sid = %form.call_sid, "circuit open — degrading gracefully");
        if session.risk.level.requires_immediate_escalation() {
            let _ = state
                .queue
                .enqueue(
                    session.id.clone(),
                    session.risk.level.0,
                    chrono::Utc::now(),
                )
                .await;
            let _ = session.transition(sayangcare_core::domain::SessionState::Escalated);
        } else {
            let _ = session.transition(sayangcare_core::domain::SessionState::Degraded);
        }
        let _ = state.sessions.put(&session).await;

        let body = r#"<?xml version="1.0" encoding="UTF-8"?>
<Response>
  <Play>/static/audio/holding.mp3</Play>
  <Redirect method="POST">/twilio/gather</Redirect>
</Response>"#;
        return xml_response(body.to_string());
    }

    // --- Real LLM path (wire your inference crate here) ---
    // This is a stub reply so the binary compiles without the inference crate.
    // Replace with: state.inference.generate_reply(&session.transcript).await
    let assistant_reply = "I hear you. Tell me more about what's on your mind.".to_string();

    session.transcript.push(Turn {
        speaker: Speaker::Assistant,
        text: assistant_reply.clone(),
        timestamp: chrono::Utc::now(),
    });

    // Persist with optimistic concurrency.
    let expected = session.version;
    session.version += 1;
    session.updated_at = chrono::Utc::now();
    if let Err(e) = state.sessions.update_cas(&session, expected).await {
        warn!(error = %e, "CAS failed on gather — retrying next turn");
    }

    // Respond with the assistant's spoken reply + gather again.
    let action = format!("{}{}", state.config.telephony.public_base_url, GATHER_ACTION);
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
        if let Ok(Some(mut session)) = state.sessions.get(&sid).await {
            let _ = session.transition(sayangcare_core::domain::SessionState::Completed);
            if let Err(e) = state.tiered.flush_to_cold(&session).await {
                warn!(error = %e, "cold flush failed");
            } else {
                info!(call_sid = %form.call_sid, "session archived");
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