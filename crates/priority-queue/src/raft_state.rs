use chrono::{DateTime, Utc};
use sayangcare_core::domain::SessionId;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Cursor;

openraft::declare_raft_types!(
    /// OpenRaft payload/response types for the volunteer escalation queue.
    pub RaftQueueTypeConfig:
        D = RaftQueueCommand,
        R = RaftQueueOutcome,
        NodeId = u64,
        Node = openraft::BasicNode,
);

/// A deterministic command that a Raft log will replicate to every queue node.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum RaftQueueCommand {
    Enqueue {
        request_id: String,
        session_id: SessionId,
        risk: u8,
        enqueued_at: DateTime<Utc>,
    },
    ClaimHighest {
        request_id: String,
        volunteer_id: String,
    },
}

impl RaftQueueCommand {
    fn request_id(&self) -> &str {
        match self {
            Self::Enqueue { request_id, .. } | Self::ClaimHighest { request_id, .. } => request_id,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum RaftQueueOutcome {
    Enqueued {
        session_id: SessionId,
        inserted: bool,
    },
    Claimed {
        session_id: Option<SessionId>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct QueueEntry {
    risk: u8,
    enqueued_at: DateTime<Utc>,
    claimed_by: Option<String>,
}

/// Deterministic state-machine logic to be applied only to committed Raft log entries.
///
/// `BTreeMap` makes snapshots and serialized state deterministic; command application is O(log n)
/// for enqueue and O(n) for finding the highest-priority unclaimed item. The latter can be replaced
/// with an ordered index after measuring queue sizes.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RaftQueueState {
    entries: BTreeMap<String, QueueEntry>,
    request_results: BTreeMap<String, RaftQueueOutcome>,
}

impl RaftQueueState {
    /// Apply a committed command. Raft serializes this call in log order on every member.
    pub fn apply(&mut self, command: RaftQueueCommand) -> RaftQueueOutcome {
        if let Some(previous) = self.request_results.get(command.request_id()) {
            return previous.clone();
        }

        let request_id = command.request_id().to_owned();
        let outcome = match command {
            RaftQueueCommand::Enqueue {
                session_id,
                risk,
                enqueued_at,
                ..
            } => {
                let inserted = if self.entries.contains_key(&session_id.0) {
                    false
                } else {
                    self.entries.insert(
                        session_id.0.clone(),
                        QueueEntry {
                            risk: risk.clamp(1, 5),
                            enqueued_at,
                            claimed_by: None,
                        },
                    );
                    true
                };
                RaftQueueOutcome::Enqueued {
                    session_id,
                    inserted,
                }
            }
            RaftQueueCommand::ClaimHighest { volunteer_id, .. } => {
                let next = self
                    .entries
                    .iter()
                    .filter(|(_, entry)| entry.claimed_by.is_none())
                    .min_by(|(left_id, left), (right_id, right)| {
                        right
                            .risk
                            .cmp(&left.risk)
                            .then_with(|| left.enqueued_at.cmp(&right.enqueued_at))
                            .then_with(|| left_id.cmp(right_id))
                    })
                    .map(|(id, _)| id.clone());

                if let Some(session_id) = &next {
                    if let Some(entry) = self.entries.get_mut(session_id) {
                        entry.claimed_by = Some(volunteer_id);
                    }
                }
                RaftQueueOutcome::Claimed {
                    session_id: next.map(SessionId),
                }
            }
        };

        self.request_results.insert(request_id, outcome.clone());
        outcome
    }

    pub fn peek_highest(&self) -> Option<SessionId> {
        self.entries
            .iter()
            .filter(|(_, entry)| entry.claimed_by.is_none())
            .min_by(|(left_id, left), (right_id, right)| {
                right
                    .risk
                    .cmp(&left.risk)
                    .then_with(|| left.enqueued_at.cmp(&right.enqueued_at))
                    .then_with(|| left_id.cmp(right_id))
            })
            .map(|(id, _)| SessionId(id.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::{RaftQueueCommand, RaftQueueOutcome, RaftQueueState};
    use chrono::{TimeZone, Utc};
    use sayangcare_core::domain::SessionId;

    fn timestamp(seconds: i64) -> chrono::DateTime<Utc> {
        Utc.timestamp_opt(seconds, 0).single().unwrap()
    }

    fn enqueue(id: &str, risk: u8, at: i64, request_id: &str) -> RaftQueueCommand {
        RaftQueueCommand::Enqueue {
            request_id: request_id.to_string(),
            session_id: SessionId(id.to_string()),
            risk,
            enqueued_at: timestamp(at),
        }
    }

    #[test]
    fn claim_orders_risk_then_fifo_then_session_id() {
        let mut state = RaftQueueState::default();
        state.apply(enqueue("CA-later", 5, 20, "enqueue-1"));
        state.apply(enqueue("CA-first-z", 5, 10, "enqueue-2"));
        state.apply(enqueue("CA-first-a", 5, 10, "enqueue-3"));
        state.apply(enqueue("CA-risk-four", 4, 1, "enqueue-4"));

        assert_eq!(
            state.peek_highest(),
            Some(SessionId("CA-first-a".to_string()))
        );
        assert_eq!(
            state.apply(RaftQueueCommand::ClaimHighest {
                request_id: "claim-1".to_string(),
                volunteer_id: "volunteer-1".to_string(),
            }),
            RaftQueueOutcome::Claimed {
                session_id: Some(SessionId("CA-first-a".to_string()))
            }
        );
        assert_eq!(
            state.peek_highest(),
            Some(SessionId("CA-first-z".to_string()))
        );
    }

    #[test]
    fn duplicate_request_returns_original_result_without_mutating_again() {
        let mut state = RaftQueueState::default();
        state.apply(enqueue("CA-one", 3, 10, "enqueue-1"));
        let claim = RaftQueueCommand::ClaimHighest {
            request_id: "claim-retry-token".to_string(),
            volunteer_id: "volunteer-1".to_string(),
        };

        let first = state.apply(claim.clone());
        let retry = state.apply(claim);

        assert_eq!(first, retry);
        assert_eq!(state.peek_highest(), None);
    }

    #[test]
    fn enqueue_retry_does_not_reset_an_existing_claim() {
        let mut state = RaftQueueState::default();
        state.apply(enqueue("CA-one", 3, 10, "enqueue-1"));
        state.apply(RaftQueueCommand::ClaimHighest {
            request_id: "claim-1".to_string(),
            volunteer_id: "volunteer-1".to_string(),
        });

        let retry = state.apply(enqueue("CA-one", 5, 0, "enqueue-retry"));

        assert_eq!(
            retry,
            RaftQueueOutcome::Enqueued {
                session_id: SessionId("CA-one".to_string()),
                inserted: false,
            }
        );
        assert_eq!(state.peek_highest(), None);
    }

    #[test]
    fn risk_is_clamped_to_domain_range_and_empty_claim_is_stable() {
        let mut state = RaftQueueState::default();
        state.apply(enqueue("CA-one", 9, 10, "enqueue-1"));
        assert_eq!(state.peek_highest(), Some(SessionId("CA-one".to_string())));
        assert_eq!(state.entries["CA-one"].risk, 5);
        state.apply(RaftQueueCommand::ClaimHighest {
            request_id: "claim-first".to_string(),
            volunteer_id: "volunteer-1".to_string(),
        });

        let empty = state.apply(RaftQueueCommand::ClaimHighest {
            request_id: "claim-empty".to_string(),
            volunteer_id: "volunteer-1".to_string(),
        });
        assert_eq!(empty, RaftQueueOutcome::Claimed { session_id: None });
    }
}
