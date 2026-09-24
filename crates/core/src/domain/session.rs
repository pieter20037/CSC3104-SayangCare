use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{RiskAssessment, SentimentSignal, Transcript};

/// Unique identifier for a voice session (maps to Twilio CallSid).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionId(pub String);

impl SessionId {
    pub fn new() -> Self {
        Self(Uuid::new_v4().to_string())
    }
}

impl std::fmt::Display for SessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Caller identity - phone number is the primary key for telehealth continuity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallerId {
    pub phone_number: String,
    pub display_name: Option<String>,
}

/// Lifecycle states for a session. Enforced via state machine transitions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    /// Call received, awaiting greeting.
    Initiated,
    /// Active conversation with LLM.
    Active,
    /// Circuit breaker tripped - playing buffered audio.
    Degraded,
    /// Escalated to human volunteer queue.
    Escalated,
    /// Call ended cleanly.
    Completed,
    /// Failed and needs recovery.
    Failed,
}

impl SessionState {
    /// Valid transitions per the SayangCare state machine.
    pub fn can_transition_to(&self, next: SessionState) -> bool {
        use SessionState::*;
        matches!(
            (self, next),
            (Initiated, Active)
                | (Initiated, Failed)
                | (Active, Degraded)
                | (Active, Escalated)
                | (Active, Completed)
                | (Active, Failed)
                | (Degraded, Active)
                | (Degraded, Escalated)
                | (Degraded, Failed)
                | (Escalated, Completed)
                | (Escalated, Failed)
        )
    }
}

/// The canonical session record. This is what gets replicated in Redis
/// and flushed to PostgreSQL on completion.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: SessionId,
    pub caller: CallerId,
    pub state: SessionState,
    pub transcript: Transcript,
    pub latest_sentiment: Option<SentimentSignal>,
    pub risk: RiskAssessment,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// Monotonic version counter for optimistic concurrency.
    pub version: u64,
}

impl Session {
    pub fn new(caller: CallerId) -> Self {
        let now = Utc::now();
        Self {
            id: SessionId::new(),
            caller,
            state: SessionState::Initiated,
            transcript: Transcript::default(),
            latest_sentiment: None,
            risk: RiskAssessment::default(),
            created_at: now,
            updated_at: now,
            version: 1,
        }
    }

    /// Transition to a new state, enforcing the state machine.
    pub fn transition(&mut self, next: SessionState) -> crate::CoreResult<()> {
        if !self.state.can_transition_to(next) {
            return Err(crate::CoreError::InvalidTransition {
                from: format!("{:?}", self.state),
                to: format!("{:?}", next),
            });
        }
        self.state = next;
        self.version += 1;
        self.updated_at = Utc::now();
        Ok(())
    }
}