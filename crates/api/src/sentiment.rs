//! CB-04 inference workflow, shared by simulated and live calls.
use sayangcare_circuit_breaker::{CircuitBreaker, RequestPermit};
use sayangcare_core::domain::{Session, Turn};
use sayangcare_core::ports::InferenceService;
use sayangcare_core::CoreResult;
use std::time::Instant;

/// Maps the domain's 1..5 score to the adaptive policy's 0..1 severity.
pub fn risk_severity(score: u8) -> f32 {
    (score.clamp(1, 5) - 1) as f32 / 4.0
}

/// Called only after a permit is acquired. Open circuits never call the model.
pub async fn generate_reply(
    inference: &dyn InferenceService,
    breaker: &CircuitBreaker,
    permit: RequestPermit,
    session: &mut Session,
    caller_turn: &Turn,
) -> CoreResult<String> {
    let started = Instant::now();
    let mut severity = risk_severity(session.risk.level.0);
    let sentiment_failed = match inference.score_sentiment(&caller_turn.text).await {
        Ok(signal) if signal.valence.is_finite() && signal.arousal.is_finite() => {
            severity = severity.max(signal.distress_severity());
            session.latest_sentiment = Some(signal);
            false
        }
        result => {
            tracing::warn!(session_id = %session.id, result = ?result, "sentiment scoring failed; using current risk severity");
            // Do not reuse a previous turn's sentiment as a fresh measurement.
            session.latest_sentiment = None;
            true
        }
    };
    let mut context = session.transcript.clone();
    context.push(caller_turn.clone());
    let reply = inference.generate_reply(&context).await;
    let latency = started.elapsed().as_millis() as u64;
    if sentiment_failed || reply.is_err() {
        breaker
            .record_failure_with_permit(permit, latency, severity)
            .await;
    } else {
        breaker
            .record_success_with_distress(permit, latency, severity)
            .await;
    }
    tracing::debug!(session_id = %session.id, severity, latency, "sentiment-adjusted inference completed");
    reply
}

#[cfg(test)]
mod tests {
    use super::*;
    use sayangcare_circuit_breaker::{BreakerConfig, CircuitState, SentimentAdaptivePolicy};
    use sayangcare_core::domain::{CallerId, RiskAssessment, SentimentSignal, Speaker, Transcript};
    use sayangcare_core::CoreError;
    use std::sync::Arc;

    struct FakeInference {
        valence: f32,
        score_fails: bool,
        reply_fails: bool,
    }
    #[async_trait::async_trait]
    impl InferenceService for FakeInference {
        async fn generate_reply(&self, context: &Transcript) -> CoreResult<String> {
            assert_eq!(context.turns.last().unwrap().text, "hello");
            if self.reply_fails {
                Err(CoreError::Inference("test reply failure".into()))
            } else {
                Ok("supportive reply".into())
            }
        }
        async fn score_sentiment(&self, _: &str) -> CoreResult<SentimentSignal> {
            if self.score_fails {
                return Err(CoreError::Inference("test score failure".into()));
            }
            Ok(SentimentSignal {
                valence: self.valence,
                arousal: 0.5,
                distress_markers: vec![],
                captured_at: chrono::Utc::now(),
            })
        }
        async fn assess_risk(
            &self,
            _: &Transcript,
            _: &SentimentSignal,
        ) -> CoreResult<RiskAssessment> {
            panic!("CB-04 must not replace deterministic safety triage")
        }
    }
    fn fixture(min_requests: u64) -> (CircuitBreaker, Session, Turn) {
        let breaker = CircuitBreaker::new(
            BreakerConfig {
                base_error_rate: 0.6,
                base_latency_ms: 10000,
                min_requests,
                half_open_after_secs: 0,
                window_size: 10,
            },
            Arc::new(SentimentAdaptivePolicy::new(0.6, 10000, 0.4)),
        );
        let session = Session::new(CallerId {
            phone_number: "test".into(),
            display_name: None,
        });
        let turn = Turn {
            speaker: Speaker::Caller,
            text: "hello".into(),
            timestamp: chrono::Utc::now(),
        };
        (breaker, session, turn)
    }

    #[test]
    fn scores_one_to_five_map_and_clamp_to_unit_range() {
        assert_eq!(
            (1..=5).map(risk_severity).collect::<Vec<_>>(),
            vec![0.0, 0.25, 0.5, 0.75, 1.0]
        );
        assert_eq!(risk_severity(0), 0.0);
        assert_eq!(risk_severity(255), 1.0);
    }

    #[actix_web::test]
    async fn live_sentiment_tightens_threshold_and_is_saved() {
        for (valence, expected) in [(1.0, CircuitState::Closed), (-1.0, CircuitState::Open)] {
            let (breaker, mut session, turn) = fixture(2);
            breaker
                .record_failure_with_permit(breaker.acquire_with_permit().unwrap(), 10, 0.0)
                .await;
            let permit = breaker.acquire_with_permit().unwrap();
            let fake = FakeInference {
                valence,
                score_fails: false,
                reply_fails: false,
            };
            assert_eq!(
                generate_reply(&fake, &breaker, permit, &mut session, &turn)
                    .await
                    .unwrap(),
                "supportive reply"
            );
            assert_eq!(session.latest_sentiment.unwrap().valence, valence);
            assert_eq!(breaker.state(), expected);
        }
    }

    #[actix_web::test]
    async fn scorer_failure_keeps_reply_and_clears_stale_signal() {
        let (breaker, mut session, turn) = fixture(1);
        let fake = FakeInference {
            valence: 1.0,
            score_fails: false,
            reply_fails: false,
        };
        let permit = breaker.acquire_with_permit().unwrap();
        generate_reply(&fake, &breaker, permit, &mut session, &turn)
            .await
            .unwrap();
        assert!(session.latest_sentiment.is_some());
        let fake = FakeInference {
            score_fails: true,
            ..fake
        };
        let permit = breaker.acquire_with_permit().unwrap();
        assert!(generate_reply(&fake, &breaker, permit, &mut session, &turn)
            .await
            .is_ok());
        assert!(session.latest_sentiment.is_none());
    }

    #[actix_web::test]
    async fn invalid_signal_or_reply_failure_opens_breaker() {
        for fake in [
            FakeInference {
                valence: f32::NAN,
                score_fails: false,
                reply_fails: false,
            },
            FakeInference {
                valence: 1.0,
                score_fails: false,
                reply_fails: true,
            },
        ] {
            let (breaker, mut session, turn) = fixture(1);
            let permit = breaker.acquire_with_permit().unwrap();
            let _ = generate_reply(&fake, &breaker, permit, &mut session, &turn).await;
            assert_eq!(breaker.state(), CircuitState::Open);
        }
    }
}
