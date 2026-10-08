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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum HandoffStatus {
    #[default]
    NotRequired,
    Pending,
    Assigned,
    Transferred,
    Resolved,
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
    #[serde(default)]
    pub is_simulated: bool,
    pub caller: CallerId,
    pub state: SessionState,
    pub transcript: Transcript,
    pub latest_sentiment: Option<SentimentSignal>,
    pub risk: RiskAssessment,
    pub handoff_status: HandoffStatus,
    pub assigned_volunteer_id: Option<String>,
    pub escalation_count: u8,
    pub operator_acknowledged: bool,
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
            is_simulated: false,
            caller,
            state: SessionState::Initiated,
            transcript: Transcript::default(),
            latest_sentiment: None,
            risk: RiskAssessment::default(),
            handoff_status: HandoffStatus::NotRequired,
            assigned_volunteer_id: None,
            escalation_count: 0,
            operator_acknowledged: false,
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

        let already_escalated = self.state == SessionState::Escalated;
        if self.state != SessionState::Active
            && !already_escalated
            && !self.state.can_transition_to(SessionState::Active)
        {
            return Err(crate::CoreError::InvalidTransition {
                from: format!("{:?}", self.state),
                to: format!("{:?}", SessionState::Active),
            });
        }

        if !already_escalated {
            self.state = SessionState::Active;
        }
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
        if next == SessionState::Escalated {
            self.handoff_status = HandoffStatus::Pending;
            self.assigned_volunteer_id = None;
            self.operator_acknowledged = false;
            self.escalation_count = self.escalation_count.saturating_add(1);
        }
        self.version += 1;
        self.updated_at = Utc::now();
        Ok(())
    }

    pub fn assign_volunteer(&mut self, volunteer_id: impl Into<String>) {
        let volunteer_id = volunteer_id.into();
        self.assigned_volunteer_id = Some(volunteer_id.clone());
        self.handoff_status = HandoffStatus::Assigned;
        self.updated_at = Utc::now();
        self.version += 1;
    }

    pub fn mark_simulated(&mut self) {
        self.is_simulated = true;
        self.updated_at = Utc::now();
        self.version += 1;
    }

    pub fn resolve_handoff(&mut self) {
        self.handoff_status = HandoffStatus::Resolved;
        self.updated_at = Utc::now();
        self.version += 1;
    }

    pub fn transfer_handoff(&mut self) {
        self.handoff_status = HandoffStatus::Transferred;
        self.updated_at = Utc::now();
        self.version += 1;
    }

    pub fn transfer_handoff_to(&mut self, volunteer_id: impl Into<String>) {
        self.assigned_volunteer_id = Some(volunteer_id.into());
        self.handoff_status = HandoffStatus::Transferred;
        self.updated_at = Utc::now();
        self.version += 1;
    }

    pub fn acknowledge_operator(&mut self) {
        self.operator_acknowledged = true;
        self.updated_at = Utc::now();
        self.version += 1;
    }

    pub fn record_repeated_risk(&mut self) {
        self.escalation_count = self.escalation_count.saturating_add(1);
        self.operator_acknowledged = false;
        self.updated_at = Utc::now();
        self.version += 1;
    }

    pub fn is_handoff_stale(&self, timeout: chrono::Duration) -> bool {
        matches!(
            self.handoff_status,
            HandoffStatus::Assigned | HandoffStatus::Transferred
        ) && self.updated_at + timeout <= Utc::now()
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
    fn recording_followup_turns_preserves_escalated_state() {
        let mut session = Session::with_id(SessionId("CA-test".to_string()), caller());
        session.transition(SessionState::Active).unwrap();
        session.transition(SessionState::Escalated).unwrap();

        session
            .record_turns([Turn {
                speaker: Speaker::Caller,
                text: "I still feel unsafe".to_string(),
                timestamp: Utc::now(),
            }])
            .unwrap();

        assert_eq!(session.state, SessionState::Escalated);
        assert_eq!(session.transcript.turns.len(), 1);
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

    #[test]
    fn volunteer_assignment_updates_handoff_state() {
        let mut session = Session::with_id(SessionId("CA-test".to_string()), caller());
        session.transition(SessionState::Active).unwrap();
        session.transition(SessionState::Escalated).unwrap();
        session.assign_volunteer("volunteer-42");

        assert_eq!(session.handoff_status, super::HandoffStatus::Assigned);
        assert_eq!(
            session.assigned_volunteer_id.as_deref(),
            Some("volunteer-42")
        );
    }

    #[test]
    fn resolving_handoff_marks_session_resolved() {
        let mut session = Session::with_id(SessionId("CA-test".to_string()), caller());
        session.transition(SessionState::Active).unwrap();
        session.transition(SessionState::Escalated).unwrap();
        session.assign_volunteer("volunteer-42");

        session.resolve_handoff();

        assert_eq!(session.handoff_status, super::HandoffStatus::Resolved);
        assert_eq!(
            session.assigned_volunteer_id.as_deref(),
            Some("volunteer-42")
        );
        assert_eq!(session.version, 5);
    }

    #[test]
    fn transferring_handoff_marks_session_transferred() {
        let mut session = Session::with_id(SessionId("CA-test".to_string()), caller());
        session.transition(SessionState::Active).unwrap();
        session.transition(SessionState::Escalated).unwrap();
        session.assign_volunteer("volunteer-42");
        let previous_version = session.version;

        session.transfer_handoff_to("volunteer-43");

        assert_eq!(session.handoff_status, super::HandoffStatus::Transferred);
        assert_eq!(
            session.assigned_volunteer_id.as_deref(),
            Some("volunteer-43")
        );
        assert_eq!(session.version, previous_version + 1);
    }

    #[test]
    fn stale_assigned_handoff_is_requeued() {
        let mut session = Session::with_id(SessionId("CA-test".to_string()), caller());
        session.transition(SessionState::Active).unwrap();
        session.transition(SessionState::Escalated).unwrap();
        session.assign_volunteer("volunteer-42");
        session.updated_at = Utc::now() - chrono::Duration::minutes(10);

        assert!(session.is_handoff_stale(chrono::Duration::minutes(5)));
        assert_eq!(session.handoff_status, super::HandoffStatus::Assigned);
    }

    #[test]
    fn operator_acknowledgment_marks_session_as_reviewed() {
        let mut session = Session::with_id(SessionId("CA-test".to_string()), caller());
        session.transition(SessionState::Active).unwrap();
        session.transition(SessionState::Escalated).unwrap();

        session.acknowledge_operator();

        assert!(session.operator_acknowledged);
        assert_eq!(session.version, 4);
    }

    #[test]
    fn repeated_risk_increments_escalation_counter() {
        let mut session = Session::with_id(SessionId("CA-test".to_string()), caller());
        session.transition(SessionState::Active).unwrap();
        session.transition(SessionState::Escalated).unwrap();

        session.record_repeated_risk();

        assert_eq!(session.escalation_count, 2);
        assert!(!session.operator_acknowledged);
    }
}
