//! Hexagonal architecture ports. Implementations live in sibling crates.

use async_trait::async_trait;
use chrono::{DateTime, Utc};

use crate::domain::{RiskAssessment, SentimentSignal, Session, SessionId, Transcript};
use crate::CoreResult;

/// Hot storage: replicated session state (Redis Sentinel).
#[async_trait]
pub trait SessionStore: Send + Sync {
    async fn get(&self, id: &SessionId) -> CoreResult<Option<Session>>;
    /// Atomically inserts a new session without replacing an existing call.
    async fn create_if_absent(&self, session: &Session) -> CoreResult<bool>;
    async fn put(&self, session: &Session) -> CoreResult<()>;
    /// Atomic compare-and-swap on version. Used for optimistic concurrency.
    async fn update_cas(&self, session: &Session, expected_version: u64) -> CoreResult<()>;
    async fn delete(&self, id: &SessionId) -> CoreResult<()>;
}

/// Cold storage: durable transcript + audit log (PostgreSQL).
#[async_trait]
pub trait ArchiveStore: Send + Sync {
    async fn archive(&self, session: &Session) -> CoreResult<()>;
    async fn fetch_session(&self, id: &SessionId) -> CoreResult<Option<Session>>;
}

/// LLM inference port. Circuit breaker wraps this.
#[async_trait]
pub trait InferenceService: Send + Sync {
    async fn generate_reply(&self, context: &Transcript) -> CoreResult<String>;

    async fn score_sentiment(&self, text: &str) -> CoreResult<SentimentSignal>;

    async fn assess_risk(
        &self,
        context: &Transcript,
        sentiment: &SentimentSignal,
    ) -> CoreResult<RiskAssessment>;
}

/// Telephony ingress (Twilio, future: multi-provider SIP).
#[async_trait]
pub trait TelephonyProvider: Send + Sync {
    async fn play_audio(&self, call_sid: &str, audio_url: &str) -> CoreResult<()>;
    async fn hangup(&self, call_sid: &str) -> CoreResult<()>;
    async fn redirect_to_human(&self, call_sid: &str, volunteer_id: &str) -> CoreResult<()>;
}

/// Priority queue for high-risk escalation.
#[async_trait]
pub trait PriorityQueue: Send + Sync {
    async fn enqueue(
        &self,
        session_id: SessionId,
        risk: u8,
        enqueued_at: DateTime<Utc>,
    ) -> CoreResult<()>;
    async fn peek_highest(&self) -> CoreResult<Option<SessionId>>;
    async fn claim(&self, volunteer_id: &str) -> CoreResult<Option<SessionId>>;
}
