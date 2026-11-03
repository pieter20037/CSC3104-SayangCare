use chrono::{DateTime, Utc};
use sayangcare_core::domain::SessionId;
use sayangcare_core::ownership::{AssignmentOwnership, OwnershipCommand, OwnershipDecision};
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
    CoordinateAssignment {
        request_id: String,
        session_id: SessionId,
        volunteer_id: String,
    },
    TransferAssignment {
        request_id: String,
        session_id: SessionId,
        from_volunteer_id: String,
        to_volunteer_id: String,
    },
    ReleaseAssignment {
        request_id: String,
        session_id: SessionId,
        volunteer_id: String,
    },
    Ownership(OwnershipCommand),
}

impl RaftQueueCommand {
    fn request_id(&self) -> &str {
        match self {
            Self::Ownership(command) => &command.request_id,
            Self::Enqueue { request_id, .. }
            | Self::ClaimHighest { request_id, .. }
            | Self::ClaimSession { request_id, .. }
            | Self::MoveClaim { request_id, .. }
            | Self::ReleaseClaim { request_id, .. } => request_id,
            Self::CoordinateAssignment { request_id, .. }
            | Self::TransferAssignment { request_id, .. }
            | Self::ReleaseAssignment { request_id, .. } => request_id,
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
    Ownership(OwnershipDecision),
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
    #[serde(default)]
    assignments: BTreeMap<String, String>,
    #[serde(default)]
    ownership: BTreeMap<String, AssignmentOwnership>,
    #[serde(default)]
    ownership_results: BTreeMap<String, (OwnershipCommand, OwnershipDecision)>,
    request_results: BTreeMap<String, RaftQueueOutcome>,
}

impl RaftQueueState {
    /// Apply a committed command. Raft serializes this call in log order on every member.
    pub fn apply(&mut self, command: RaftQueueCommand) -> RaftQueueOutcome {
        if let RaftQueueCommand::Ownership(command) = &command {
            return RaftQueueOutcome::Ownership(self.apply_ownership(command.clone()));
        }
        if let Some(previous) = self.request_results.get(command.request_id()) {
            return previous.clone();
        }

        let request_id = command.request_id().to_owned();
        let outcome = match command {
            RaftQueueCommand::Ownership(_) => unreachable!("ownership handled before legacy dedup"),
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
            RaftQueueCommand::CoordinateAssignment {
                session_id,
                volunteer_id,
                ..
            } => {
                let ok = if self.assignments.contains_key(&session_id.0) {
                    false
                } else {
                    self.assignments.insert(session_id.0, volunteer_id);
                    true
                };
                RaftQueueOutcome::Applied { ok }
            }
            RaftQueueCommand::TransferAssignment {
                session_id,
                from_volunteer_id,
                to_volunteer_id,
                ..
            } => {
                let ok = match self.assignments.get_mut(&session_id.0) {
                    Some(owner) if owner == &from_volunteer_id => {
                        *owner = to_volunteer_id;
                        true
                    }
                    _ => false,
                };
                RaftQueueOutcome::Applied { ok }
            }
            RaftQueueCommand::ReleaseAssignment {
                session_id,
                volunteer_id,
                ..
            } => {
                let ok = self.assignments.get(&session_id.0) == Some(&volunteer_id);
                if ok {
                    self.assignments.remove(&session_id.0);
                }
                RaftQueueOutcome::Applied { ok }
            }
        };

        self.request_results.insert(request_id, outcome.clone());
        // Retry tokens only need to outlive client retries, so keep the table bounded.
        while self.request_results.len() > MAX_REMEMBERED_REQUESTS {
            self.request_results.pop_first();
        }
        outcome
    }

    fn apply_ownership(&mut self, command: OwnershipCommand) -> OwnershipDecision {
        if let Some((previous, decision)) = self.ownership_results.get(&command.request_id) {
            return if previous == &command {
                decision.clone()
            } else {
                OwnershipDecision::RequestIdConflict
            };
        }
        let current = self
            .ownership
            .get(&command.session_id.0)
            .cloned()
            .unwrap_or_default();
        let decision = current.decide(&command);
        if let OwnershipDecision::Applied { ownership } = &decision {
            self.ownership
                .insert(command.session_id.0.clone(), ownership.clone());
        }
        // Recovery retries stable request ids even after compaction and long outages.
        self.ownership_results
            .insert(command.request_id.clone(), (command, decision.clone()));
        decision
    }

    pub fn ownership_snapshot(&self) -> BTreeMap<String, AssignmentOwnership> {
        self.ownership.clone()
    }

    pub fn legacy_assignments_present(&self) -> bool {
        !self.assignments.is_empty()
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
    use sayangcare_core::ownership::{
        AssignmentOwnership, OwnershipCommand, OwnershipDecision, OwnershipTransition,
        MAX_ASSIGNMENT_GENERATION,
    };

    fn ownership(
        request_id: &str,
        generation: u64,
        transition: OwnershipTransition,
    ) -> RaftQueueCommand {
        RaftQueueCommand::Ownership(OwnershipCommand {
            request_id: request_id.into(),
            session_id: SessionId("session".into()),
            expected_generation: generation,
            transition,
        })
    }

    fn claim(request_id: &str, generation: u64, volunteer: &str) -> RaftQueueCommand {
        ownership(
            request_id,
            generation,
            OwnershipTransition::Claim {
                volunteer_id: volunteer.into(),
            },
        )
    }

    fn applied(volunteer: Option<&str>, generation: u64) -> RaftQueueOutcome {
        RaftQueueOutcome::Ownership(OwnershipDecision::Applied {
            ownership: AssignmentOwnership {
                volunteer_id: volunteer.map(str::to_owned),
                assignment_generation: generation,
            },
        })
    }

    #[test]
    fn ownership_races_follow_log_order_and_reject_stale_generation() {
        for (first, second) in [("aisha", "noor"), ("noor", "aisha")] {
            let mut state = RaftQueueState::default();
            assert_eq!(
                state.apply(claim("first", 0, first)),
                applied(Some(first), 1)
            );
            assert_eq!(
                state.apply(claim("second", 0, second)),
                RaftQueueOutcome::Ownership(OwnershipDecision::StaleGeneration)
            );
            assert_eq!(
                state.apply(claim("occupied", 1, second)),
                RaftQueueOutcome::Ownership(OwnershipDecision::Conflict)
            );
            assert_eq!(
                state.ownership_snapshot()["session"]
                    .volunteer_id
                    .as_deref(),
                Some(first)
            );
        }
    }

    #[test]
    fn ownership_transfer_release_and_reclaim_never_reset_generation() {
        let mut state = RaftQueueState::default();
        assert_eq!(
            state.apply(claim("claim", 0, "aisha")),
            applied(Some("aisha"), 1)
        );
        assert_eq!(
            state.apply(ownership(
                "transfer",
                1,
                OwnershipTransition::Transfer {
                    from_volunteer_id: "aisha".into(),
                    to_volunteer_id: "noor".into(),
                }
            )),
            applied(Some("noor"), 2)
        );
        assert_eq!(
            state.apply(ownership(
                "wrong-owner",
                2,
                OwnershipTransition::Release {
                    volunteer_id: "aisha".into(),
                    resolved: false,
                }
            )),
            RaftQueueOutcome::Ownership(OwnershipDecision::Conflict)
        );
        assert_eq!(
            state.apply(ownership(
                "release",
                2,
                OwnershipTransition::Release {
                    volunteer_id: "noor".into(),
                    resolved: true,
                }
            )),
            applied(None, 3)
        );
        assert_eq!(
            state.ownership_snapshot()["session"].assignment_generation,
            3
        );
        assert_eq!(
            state.apply(claim("stale-reclaim", 0, "aisha")),
            RaftQueueOutcome::Ownership(OwnershipDecision::StaleGeneration)
        );
        assert_eq!(
            state.apply(claim("reclaim", 3, "aisha")),
            applied(Some("aisha"), 4)
        );
    }

    #[test]
    fn ownership_dedup_retains_original_payload_and_failed_decisions() {
        let mut state = RaftQueueState::default();
        let original = claim("stable", 0, "aisha");
        let expected = state.apply(original.clone());
        let stale = claim("stale", 0, "noor");
        let stale_result = state.apply(stale.clone());
        assert_eq!(
            state.apply(claim("stable", 1, "noor")),
            RaftQueueOutcome::Ownership(OwnershipDecision::RequestIdConflict)
        );
        for index in 0..=super::MAX_REMEMBERED_REQUESTS {
            state.apply(claim(&format!("later-{index}"), 0, "noor"));
        }
        let bytes = serde_json::to_vec(&state).unwrap();
        let mut restored: RaftQueueState = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(restored.apply(original), expected);
        assert_eq!(restored.apply(stale), stale_result);
        assert_eq!(
            restored.ownership_snapshot()["session"].assignment_generation,
            1
        );
        assert!(restored.ownership_results.len() > super::MAX_REMEMBERED_REQUESTS);
    }

    #[test]
    fn ownership_rejects_all_request_payload_changes() {
        let original = ownership(
            "stable",
            1,
            OwnershipTransition::Release {
                volunteer_id: "aisha".into(),
                resolved: false,
            },
        );
        let mut state = RaftQueueState::default();
        state.apply(claim("claim", 0, "aisha"));
        let expected = state.apply(original.clone());
        let RaftQueueCommand::Ownership(command) = &original else {
            unreachable!()
        };
        let mut changed_session = command.clone();
        changed_session.session_id = SessionId("other".into());
        let mut changed_generation = command.clone();
        changed_generation.expected_generation = 2;
        let mut changed_resolution = command.clone();
        changed_resolution.transition = OwnershipTransition::Release {
            volunteer_id: "aisha".into(),
            resolved: true,
        };
        for changed in [changed_session, changed_generation, changed_resolution] {
            assert_eq!(
                state.apply(RaftQueueCommand::Ownership(changed)),
                RaftQueueOutcome::Ownership(OwnershipDecision::RequestIdConflict)
            );
        }
        assert_eq!(state.apply(original), expected);
    }

    #[test]
    fn ownership_generation_exhaustion_and_legacy_snapshot_compatibility() {
        let mut state: RaftQueueState = serde_json::from_value(serde_json::json!({
            "entries": {}, "assignments": {"old": "aisha"}, "request_results": {}
        }))
        .unwrap();
        assert!(state.legacy_assignments_present());
        assert!(state.ownership_snapshot().is_empty());
        state.ownership.insert(
            "session".into(),
            AssignmentOwnership {
                volunteer_id: None,
                assignment_generation: MAX_ASSIGNMENT_GENERATION,
            },
        );
        assert_eq!(
            state.apply(claim("exhausted", MAX_ASSIGNMENT_GENERATION, "aisha")),
            RaftQueueOutcome::Ownership(OwnershipDecision::GenerationExhausted)
        );
        assert_eq!(
            state.ownership_snapshot()["session"].assignment_generation,
            MAX_ASSIGNMENT_GENERATION
        );
    }

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
