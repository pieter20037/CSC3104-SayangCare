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
                | (Initiated, Completed)
                | (Initiated, Failed)
                | (Active, Degraded)
                | (Active, Escalated)
                | (Active, Completed)
                | (Active, Failed)
                | (Degraded, Active)
                | (Degraded, Escalated)
                | (Degraded, Completed)
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
        Self::with_id(SessionId::new(), caller)
    }

    pub fn with_id(id: SessionId, caller: CallerId) -> Self {
        let now = Utc::now();
        Self {
            id,
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

    /// Appends a webhook's turns as one versioned mutation; work is O(turns), and callers persist it with CAS.
    pub fn record_turns(
        &mut self,
        turns: impl IntoIterator<Item = super::Turn>,
    ) -> crate::CoreResult<()> {
        let turns: Vec<_> = turns.into_iter().collect();
        if turns.is_empty() {
            return Ok(());
        }

        if self.state != SessionState::Active && !self.state.can_transition_to(SessionState::Active)
        {
            return Err(crate::CoreError::InvalidTransition {
                from: format!("{:?}", self.state),
                to: format!("{:?}", SessionState::Active),
            });
        }

        self.state = SessionState::Active;
        self.transcript.turns.extend(turns);
        self.version += 1;
        self.updated_at = Utc::now();
        Ok(())
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

#[cfg(test)]
mod tests {
    use super::{CallerId, Session, SessionId, SessionState};
    use crate::domain::{Speaker, Turn};
    use chrono::Utc;

    fn caller() -> CallerId {
        CallerId {
            phone_number: "+6500000000".to_string(),
            display_name: None,
        }
    }

    #[test]
    fn supplied_call_id_is_preserved() {
        let session = Session::with_id(SessionId("CA-test".to_string()), caller());

        assert_eq!(session.id.0, "CA-test");
        assert_eq!(session.version, 1);
    }

    #[test]
    fn recording_turns_activates_session_and_increments_version_once() {
        let mut session = Session::with_id(SessionId("CA-test".to_string()), caller());
        let updated_at = session.updated_at;

        session
            .record_turns([
                Turn {
                    speaker: Speaker::Caller,
                    text: "Hello".to_string(),
                    timestamp: Utc::now(),
                },
                Turn {
                    speaker: Speaker::Assistant,
                    text: "Hi".to_string(),
                    timestamp: Utc::now(),
                },
            ])
            .unwrap();

        assert_eq!(session.state, SessionState::Active);
        assert_eq!(session.version, 2);
        assert_eq!(session.transcript.turns.len(), 2);
        assert!(session.updated_at >= updated_at);
    }

    #[test]
    fn completed_session_rejects_new_turns_without_mutation() {
        let mut session = Session::with_id(SessionId("CA-test".to_string()), caller());
        session.transition(SessionState::Active).unwrap();
        session.transition(SessionState::Completed).unwrap();
        let version = session.version;

        let result = session.record_turns([Turn {
            speaker: Speaker::Caller,
            text: "Late turn".to_string(),
            timestamp: Utc::now(),
        }]);

        assert!(result.is_err());
        assert_eq!(session.version, version);
        assert!(session.transcript.turns.is_empty());
    }
}
