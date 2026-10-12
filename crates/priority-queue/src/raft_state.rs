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
    ClaimSession {
        request_id: String,
        session_id: SessionId,
        volunteer_id: String,
    },
    MoveClaim {
        request_id: String,
        session_id: SessionId,
        from_volunteer_id: String,
        to_volunteer_id: String,
    },
    ReleaseClaim {
        request_id: String,
        session_id: SessionId,
        volunteer_id: String,
    },
}

impl RaftQueueCommand {
    fn request_id(&self) -> &str {
        match self {
            Self::Enqueue { request_id, .. }
            | Self::ClaimHighest { request_id, .. }
            | Self::ClaimSession { request_id, .. }
            | Self::MoveClaim { request_id, .. }
            | Self::ReleaseClaim { request_id, .. } => request_id,
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
    /// Result of claim-by-id, move, and release commands.
    Applied {
        ok: bool,
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
            RaftQueueCommand::ClaimSession {
                session_id,
                volunteer_id,
                ..
            } => {
                let ok = match self.entries.get_mut(&session_id.0) {
                    Some(entry) if entry.claimed_by.is_none() => {
                        entry.claimed_by = Some(volunteer_id);
                        true
                    }
                    _ => false,
                };
                RaftQueueOutcome::Applied { ok }
            }
            RaftQueueCommand::MoveClaim {
                session_id,
                from_volunteer_id,
                to_volunteer_id,
                ..
            } => {
                let ok = match self.entries.get_mut(&session_id.0) {
                    Some(entry) if entry.claimed_by.as_deref() == Some(&from_volunteer_id) => {
                        entry.claimed_by = Some(to_volunteer_id);
                        true
                    }
                    _ => false,
                };
                RaftQueueOutcome::Applied { ok }
            }
            RaftQueueCommand::ReleaseClaim {
                session_id,
                volunteer_id,
                ..
            } => {
                let owned = self
                    .entries
                    .get(&session_id.0)
                    .is_some_and(|entry| entry.claimed_by.as_deref() == Some(&volunteer_id));
                if owned {
                    self.entries.remove(&session_id.0);
                }
                RaftQueueOutcome::Applied { ok: owned }
            }
        };

        self.request_results.insert(request_id, outcome.clone());
        // Retry tokens only need to outlive client retries, so keep the table bounded.
        while self.request_results.len() > MAX_REMEMBERED_REQUESTS {
            self.request_results.pop_first();
        }
        outcome
    }

    fn pending_ordered(&self) -> Vec<&String> {
        let mut pending: Vec<_> = self
            .entries
            .iter()
            .filter(|(_, entry)| entry.claimed_by.is_none())
            .collect();
        pending.sort_by(|(left_id, left), (right_id, right)| {
            right
                .risk
                .cmp(&left.risk)
                .then_with(|| left.enqueued_at.cmp(&right.enqueued_at))
                .then_with(|| left_id.cmp(right_id))
        });
        pending.into_iter().map(|(id, _)| id).collect()
    }

    pub fn peek_highest(&self) -> Option<SessionId> {
        self.pending_ordered()
            .first()
            .map(|id| SessionId((*id).clone()))
    }

    pub fn list_pending(&self) -> Vec<SessionId> {
        self.pending_ordered()
            .into_iter()
            .map(|id| SessionId(id.clone()))
            .collect()
    }

    pub fn list_claimed(&self, volunteer_id: &str) -> Vec<SessionId> {
        self.entries
            .iter()
            .filter(|(_, entry)| entry.claimed_by.as_deref() == Some(volunteer_id))
            .map(|(id, _)| SessionId(id.clone()))
            .collect()
    }
}

const MAX_REMEMBERED_REQUESTS: usize = 10_000;

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
    fn claim_move_and_release_enforce_ownership() {
        let mut state = RaftQueueState::default();
        state.apply(enqueue("CA-one", 4, 10, "enqueue-1"));
        let applied = |ok| RaftQueueOutcome::Applied { ok };
        let claim = |request: &str, volunteer: &str| RaftQueueCommand::ClaimSession {
            request_id: request.to_string(),
            session_id: SessionId("CA-one".to_string()),
            volunteer_id: volunteer.to_string(),
        };

        assert_eq!(state.apply(claim("c1", "aisha")), applied(true));
        assert_eq!(state.apply(claim("c2", "noor")), applied(false));
        assert!(state.list_pending().is_empty());
        assert_eq!(state.list_claimed("aisha").len(), 1);

        let move_claim = |request: &str, from: &str, to: &str| RaftQueueCommand::MoveClaim {
            request_id: request.to_string(),
            session_id: SessionId("CA-one".to_string()),
            from_volunteer_id: from.to_string(),
            to_volunteer_id: to.to_string(),
        };
        assert_eq!(
            state.apply(move_claim("m1", "noor", "ibrahim")),
            applied(false)
        );
        assert_eq!(
            state.apply(move_claim("m2", "aisha", "noor")),
            applied(true)
        );

        let release = |request: &str, volunteer: &str| RaftQueueCommand::ReleaseClaim {
            request_id: request.to_string(),
            session_id: SessionId("CA-one".to_string()),
            volunteer_id: volunteer.to_string(),
        };
        assert_eq!(state.apply(release("r1", "aisha")), applied(false));
        assert_eq!(state.apply(release("r2", "noor")), applied(true));

        // Released sessions can be queued again.
        state.apply(enqueue("CA-one", 4, 20, "enqueue-2"));
        assert_eq!(state.list_pending().len(), 1);
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
