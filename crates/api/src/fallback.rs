//! CB-03: cached holding audio, independent of the inference provider.
use actix_web::{http::header, HttpResponse};
use sayangcare_core::domain::{Session, SessionState};

pub const AUDIO_PATH: &str = "/audio/holding-v1.wav";
pub const HOLDING_MESSAGE: &str = "I'm still here with you. Our service is taking a little longer to respond. Please stay on the line. You can tell me how you are feeling after the tone.";
const AUDIO: &[u8] = include_bytes!("../assets/holding-v1.wav");

/// Versioned URL lets Twilio cache the recording; no disk or AI call at runtime.
pub async fn audio() -> HttpResponse {
    HttpResponse::Ok()
        .insert_header((header::CONTENT_TYPE, "audio/wav"))
        .insert_header((header::CACHE_CONTROL, "public, max-age=31536000, immutable"))
        .body(AUDIO)
}

pub fn audio_url(base: &str) -> String {
    format!("{}{AUDIO_PATH}", base.trim_end_matches('/'))
}

/// Call after record_turns, which restores Degraded sessions to Active.
pub fn mark_degraded(session: &mut Session, use_audio: bool) -> sayangcare_core::CoreResult<()> {
    if use_audio {
        session.transition(SessionState::Degraded)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sayangcare_core::domain::{CallerId, Speaker, Turn};

    #[actix_web::test]
    async fn cached_audio_is_a_valid_wav_response() {
        let response = audio().await;
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "audio/wav"
        );
        assert_eq!(
            response.headers().get(header::CACHE_CONTROL).unwrap(),
            "public, max-age=31536000, immutable"
        );
        let bytes = actix_web::body::to_bytes(response.into_body())
            .await
            .unwrap();
        assert_eq!(&bytes[..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WAVE");
        assert!(bytes.len() > 44);
    }

    #[test]
    fn degraded_session_recovers_and_can_escalate() {
        let mut session = Session::new(CallerId {
            phone_number: "test".into(),
            display_name: None,
        });
        let turn = || Turn {
            speaker: Speaker::Caller,
            text: "hello".into(),
            timestamp: chrono::Utc::now(),
        };
        session.record_turns([turn()]).unwrap();
        mark_degraded(&mut session, true).unwrap();
        assert_eq!(session.state, SessionState::Degraded);
        session.record_turns([turn()]).unwrap();
        mark_degraded(&mut session, false).unwrap();
        assert_eq!(session.state, SessionState::Active);
        mark_degraded(&mut session, true).unwrap();
        session.transition(SessionState::Escalated).unwrap();
        mark_degraded(&mut session, false).unwrap();
        assert_eq!(session.state, SessionState::Escalated);
    }

    #[test]
    fn url_handles_trailing_slash() {
        assert_eq!(
            audio_url("https://example.com/"),
            "https://example.com/audio/holding-v1.wav"
        );
    }
}
