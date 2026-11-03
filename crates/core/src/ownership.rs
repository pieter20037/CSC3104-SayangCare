use crate::{domain::SessionId, CoreResult};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

pub const MAX_ASSIGNMENT_GENERATION: u64 = i64::MAX as u64;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssignmentOwnership {
    pub volunteer_id: Option<String>,
    pub assignment_generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OwnershipTransition {
    Claim {
        volunteer_id: String,
    },
    Transfer {
        from_volunteer_id: String,
        to_volunteer_id: String,
    },
    Release {
        volunteer_id: String,
        resolved: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnershipCommand {
    pub request_id: String,
    pub session_id: SessionId,
    pub expected_generation: u64,
    pub transition: OwnershipTransition,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum OwnershipDecision {
    Applied { ownership: AssignmentOwnership },
    StaleGeneration,
    Conflict,
    GenerationExhausted,
    RequestIdConflict,
}

impl AssignmentOwnership {
    pub fn decide(&self, command: &OwnershipCommand) -> OwnershipDecision {
        if self.assignment_generation != command.expected_generation {
            return OwnershipDecision::StaleGeneration;
        }
        let next_owner = match &command.transition {
            OwnershipTransition::Claim { volunteer_id } if self.volunteer_id.is_none() => {
                Some(volunteer_id.clone())
            }
            OwnershipTransition::Transfer {
                from_volunteer_id,
                to_volunteer_id,
            } if self.volunteer_id.as_ref() == Some(from_volunteer_id)
                && from_volunteer_id != to_volunteer_id =>
            {
                Some(to_volunteer_id.clone())
            }
            OwnershipTransition::Release { volunteer_id, .. }
                if self.volunteer_id.as_ref() == Some(volunteer_id) =>
            {
                None
            }
            _ => return OwnershipDecision::Conflict,
        };
        let Some(generation) = self
            .assignment_generation
            .checked_add(1)
            .filter(|value| *value <= MAX_ASSIGNMENT_GENERATION)
        else {
            return OwnershipDecision::GenerationExhausted;
        };
        OwnershipDecision::Applied {
            ownership: Self {
                volunteer_id: next_owner,
                assignment_generation: generation,
            },
        }
    }
}

#[async_trait]
pub trait OwnershipCoordinator: Send + Sync {
    async fn commit(&self, command: OwnershipCommand) -> CoreResult<OwnershipDecision>;
}
