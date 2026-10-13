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
    let mut session = Session::with_id(
        SessionId(form.call_sid.clone()),
        CallerId {
            phone_number: form.from.clone(),
            display_name: None,
        },
    );

    match state.sessions.create_if_absent(&session).await {
        Ok(true) => {}
        Ok(false) => match state.sessions.get(&session.id).await {
            Ok(Some(existing)) if existing.caller.phone_number == form.from => session = existing,
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

    // Observe the breaker without acquiring a permit: the greeting makes no
    // inference request and must not consume the Half-Open recovery slot.
    let breaker = state.breakers.get("llm-inference").await;
    if incoming_uses_audio(&session, state.inference.is_some(), breaker.state()) {
        if session.state != sayangcare_core::domain::SessionState::Degraded {
            let expected = session.version;
            if session
                .record_turns([Turn {
                    speaker: Speaker::Assistant,
                    text: crate::fallback::HOLDING_MESSAGE.to_string(),
                    timestamp: chrono::Utc::now(),
                }])
                .is_err()
                || crate::fallback::mark_degraded(&mut session, true).is_err()
            {
                return twiml_error("Your session cannot continue in its current state.");
            }
            if let Err(error) = state.sessions.update_cas(&session, expected).await {
                warn!(error = %error, "failed to persist incoming audio fallback");
                return twiml_error("Your session changed concurrently. Please try again.");
            }
        }
        return xml_response(reply_twiml(
            &action,
            crate::fallback::HOLDING_MESSAGE,
            Some(&crate::fallback::audio_url(
                &state.config.telephony.public_base_url,
            )),
        ));
    }
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

fn incoming_uses_audio(
    session: &Session,
    inference_configured: bool,
    breaker_state: sayangcare_circuit_breaker::CircuitState,
) -> bool {
    use sayangcare_core::domain::SessionState;
    inference_configured
        && breaker_state == sayangcare_circuit_breaker::CircuitState::Open
        && matches!(
            session.state,
            SessionState::Initiated | SessionState::Active | SessionState::Degraded
        )
        && !session.risk.level.requires_immediate_escalation()
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
    let mut use_audio = false;
    let assistant_reply = if escalated {
        "Thank you for telling me. I'm sorry you're going through this. I want to help keep you safe, and I'm alerting a human volunteer now. Are you in immediate danger right now?".to_string()
    } else if let Some(inference) = state.inference.as_ref() {
        let breaker = state.breakers.get("llm-inference").await;

        match breaker.acquire_with_permit() {
            Err(_) => {
                // Circuit is Open, or another recovery probe is running.
                use_audio = true;
                crate::fallback::HOLDING_MESSAGE.to_string()
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
                            call_sid = %form.call_sid,
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

    if crate::fallback::mark_degraded(&mut session, use_audio).is_err() {
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
    xml_response(reply_twiml(
        &action,
        &assistant_reply,
        use_audio
            .then(|| crate::fallback::audio_url(&state.config.telephony.public_base_url))
            .as_deref(),
    ))
}

fn reply_twiml(action: &str, reply: &str, audio_url: Option<&str>) -> String {
    let action = xml_escape(action);
    let escaped = xml_escape(reply);
    let playback = audio_url
        .map(|url| format!("<Play>{}</Play>", xml_escape(url)))
        .unwrap_or_default();
    let body = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<Response>
  <Say voice="Polly.Joanna">{escaped}</Say>
  {playback}
  <Gather input="speech" speechTimeout="auto" action="{action}" method="POST" actionOnEmptyResult="true">
    <Say voice="Polly.Joanna">I'm still here.</Say>
  </Gather>
  <Redirect method="POST">{action}</Redirect>
</Response>"#
    );
    body
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn incoming_audio_only_when_open_and_preserves_escalation() {
        use sayangcare_circuit_breaker::CircuitState;
        use sayangcare_core::domain::SessionState;
        let mut session = Session::new(CallerId {
            phone_number: "test".into(),
            display_name: None,
        });
        assert!(incoming_uses_audio(&session, true, CircuitState::Open));
        assert!(!incoming_uses_audio(&session, true, CircuitState::Closed));
        assert!(!incoming_uses_audio(&session, true, CircuitState::HalfOpen));
        assert!(!incoming_uses_audio(&session, false, CircuitState::Open));
        session.state = SessionState::Escalated;
        assert!(!incoming_uses_audio(&session, true, CircuitState::Open));
        session.state = SessionState::Completed;
        assert!(!incoming_uses_audio(&session, true, CircuitState::Open));
        session.state = SessionState::Active;
        session.risk = sayangcare_core::domain::RiskAssessment::from_text("I want to kill myself");
        assert!(session.risk.level.requires_immediate_escalation());
        assert!(!incoming_uses_audio(&session, true, CircuitState::Open));
    }

    #[actix_web::test]
    async fn open_breaker_routes_to_audio_and_recovery_returns_to_speech() {
        use sayangcare_circuit_breaker::{BreakerConfig, CircuitBreaker, SentimentAdaptivePolicy};
        let breaker = CircuitBreaker::new(
            BreakerConfig {
                base_error_rate: 0.25,
                base_latency_ms: 2000,
                min_requests: 1,
                half_open_after_secs: 0,
                window_size: 10,
            },
            Arc::new(SentimentAdaptivePolicy::new(0.25, 2000, 0.5)),
        );
        let permit = breaker.acquire_with_permit().unwrap();
        breaker.record_failure_with_permit(permit, 100, 0.25).await;
        let use_audio = breaker.acquire_with_permit().is_err();
        assert!(use_audio);
        let url = crate::fallback::audio_url("https://example.com");
        let xml = reply_twiml(
            "/twilio/gather",
            crate::fallback::HOLDING_MESSAGE,
            use_audio.then_some(url.as_str()),
        );
        assert!(xml.contains("<Play>"));
        breaker.tick().await;
        let probe = breaker.acquire_with_permit().unwrap();
        assert!(breaker.acquire_with_permit().is_err());
        breaker.record_success_with_permit(probe, 100);
        assert!(breaker.acquire_with_permit().is_ok());
        assert!(!reply_twiml("/twilio/gather", "I'm listening", None).contains("<Play>"));
    }

    #[test]
    fn holding_audio_preserves_gather_and_does_not_hang_up() {
        let xml = reply_twiml(
            "https://example.com/twilio/gather",
            crate::fallback::HOLDING_MESSAGE,
            Some("https://example.com/audio/holding-v1.wav"),
        );
        assert!(xml.contains("<Play>https://example.com/audio/holding-v1.wav</Play>"));
        assert!(xml.contains("<Gather input=\"speech\""));
        assert!(xml.contains("actionOnEmptyResult=\"true\""));
        assert!(xml.contains("<Redirect method=\"POST\">"));
        assert!(!xml.contains("<Hangup"));
    }

    #[test]
    fn normal_reply_still_uses_say_and_escapes_xml() {
        let xml = reply_twiml(
            "https://example.com/gather?a=1&b=2",
            "I'm listening & here",
            None,
        );
        assert!(!xml.contains("<Play>"));
        assert!(xml.contains("I&apos;m listening &amp; here"));
        assert!(xml.contains("a=1&amp;b=2"));
    }
}
