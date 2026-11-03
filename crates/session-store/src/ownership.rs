use chrono::Utc;
use sayangcare_core::domain::{HandoffStatus, Session, SessionState};
use sayangcare_core::ownership::{
    AssignmentOwnership, OwnershipCommand, OwnershipCoordinator, OwnershipDecision,
    OwnershipTransition, MAX_ASSIGNMENT_GENERATION,
};
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Postgres, Row, Transaction};
use std::sync::Arc;

#[cfg(test)]
#[path = "ownership_tests.rs"]
mod tests;

#[derive(Debug, thiserror::Error)]
pub enum OwnershipError {
    #[error("Stale assignment generation (fencing token rejected)")]
    Stale,
    #[error("case ownership conflicts with this request")]
    Conflict,
    #[error("ownership operation pending; retry with the same request_id")]
    Pending,
    #[error("case not found")]
    NotFound,
    #[error("{0}")]
    Invalid(String),
    #[error("ownership storage/coordinator unavailable: {0}")]
    Unavailable(String),
}

fn unavailable(error: impl std::fmt::Display) -> OwnershipError {
    OwnershipError::Unavailable(error.to_string())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OwnershipView {
    pub volunteer_id: Option<String>,
    pub assignment_generation: u64,
    pub pending_operation_id: Option<String>,
    pub coordinator: String,
}

pub struct OwnershipStore {
    pool: PgPool,
    coordinator: Option<Arc<dyn OwnershipCoordinator>>,
}

impl OwnershipStore {
    pub fn new(pool: PgPool, coordinator: Option<Arc<dyn OwnershipCoordinator>>) -> Self {
        Self { pool, coordinator }
    }

    pub fn mode(&self) -> &'static str {
        if self.coordinator.is_some() {
            "raft"
        } else {
            "postgres"
        }
    }

    pub async fn current(&self, id: &str) -> Result<OwnershipView, OwnershipError> {
        let row = sqlx::query("SELECT volunteer_id, assignment_generation, pending_operation_id, coordinator FROM case_ownership WHERE session_id=$1")
            .bind(id).fetch_optional(&self.pool).await.map_err(unavailable)?;
        match row {
            Some(row) => Ok(OwnershipView {
                volunteer_id: row.get("volunteer_id"),
                assignment_generation: u64::try_from(row.get::<i64, _>("assignment_generation"))
                    .map_err(unavailable)?,
                pending_operation_id: row.get("pending_operation_id"),
                coordinator: row.get("coordinator"),
            }),
            None => Ok(OwnershipView {
                volunteer_id: None,
                assignment_generation: 0,
                pending_operation_id: None,
                coordinator: self.mode().into(),
            }),
        }
    }

    pub async fn execute(
        &self,
        command: OwnershipCommand,
    ) -> Result<OwnershipDecision, OwnershipError> {
        validate_command(&command)?;
        let encoded = serde_json::to_value(&command).map_err(unavailable)?;
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        sqlx::query("INSERT INTO case_ownership(session_id, coordinator) VALUES($1,$2) ON CONFLICT DO NOTHING")
            .bind(&command.session_id.0).bind(self.mode()).execute(&mut *tx).await.map_err(unavailable)?;
        let row = sqlx::query("SELECT volunteer_id,assignment_generation,pending_operation_id,coordinator FROM case_ownership WHERE session_id=$1 FOR UPDATE")
            .bind(&command.session_id.0).fetch_one(&mut *tx).await.map_err(unavailable)?;
        let generation =
            u64::try_from(row.get::<i64, _>("assignment_generation")).map_err(unavailable)?;
        let owner: Option<String> = row.get("volunteer_id");
        let pending: Option<String> = row.get("pending_operation_id");
        let mode: String = row.get("coordinator");
        if generation > 0 && mode != self.mode() {
            return Err(OwnershipError::Invalid("ownership backend differs from retained history; explicit offline migration required".into()));
        }
        let existing =
            sqlx::query("SELECT command,outcome FROM assignment_operations WHERE request_id=$1")
                .bind(&command.request_id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(unavailable)?;
        if let Some(existing) = existing {
            if existing.get::<serde_json::Value, _>("command") != encoded {
                return Err(OwnershipError::Conflict);
            }
            if let Some(outcome) = existing.get::<Option<serde_json::Value>, _>("outcome") {
                return serde_json::from_value(outcome).map_err(unavailable);
            }
        } else {
            if pending.is_some() {
                return Err(OwnershipError::Conflict);
            }
            if generation != command.expected_generation {
                return Err(OwnershipError::Stale);
            }
            let current = AssignmentOwnership {
                volunteer_id: owner,
                assignment_generation: generation,
            };
            if !matches!(current.decide(&command), OwnershipDecision::Applied { .. }) {
                return Ok(current.decide(&command));
            }
            let snapshot: Option<serde_json::Value> = sqlx::query_scalar(
                "SELECT snapshot FROM session_checkpoints WHERE id=$1 FOR UPDATE",
            )
            .bind(&command.session_id.0)
            .fetch_optional(&mut *tx)
            .await
            .map_err(unavailable)?;
            let session: Session =
                serde_json::from_value(snapshot.ok_or(OwnershipError::NotFound)?)
                    .map_err(unavailable)?;
            if matches!(command.transition, OwnershipTransition::Claim { .. })
                && (session.state != SessionState::Escalated
                    || session.handoff_status != HandoffStatus::Pending)
            {
                return Err(OwnershipError::Conflict);
            }
            sqlx::query(
                "INSERT INTO assignment_operations(request_id,session_id,command) VALUES($1,$2,$3)",
            )
            .bind(&command.request_id)
            .bind(&command.session_id.0)
            .bind(encoded)
            .execute(&mut *tx)
            .await
            .map_err(unavailable)?;
            sqlx::query("UPDATE case_ownership SET pending_operation_id=$2,coordinator=$3,updated_at=now() WHERE session_id=$1")
                .bind(&command.session_id.0).bind(&command.request_id).bind(self.mode())
                .execute(&mut *tx).await.map_err(unavailable)?;
        }
        if self.coordinator.is_none() {
            let decision = AssignmentOwnership {
                volunteer_id: row.get("volunteer_id"),
                assignment_generation: generation,
            }
            .decide(&command);
            self.install(&mut tx, &command, &decision).await?;
            tx.commit().await.map_err(unavailable)?;
            return Ok(decision);
        }
        tx.commit().await.map_err(unavailable)?;
        let decision = self.coordinator.as_ref().expect("checked coordinator")
            .commit(command.clone()).await.map_err(|error| {
                tracing::error!(%error, request_id=%command.request_id, "ownership intent retained for recovery");
                unavailable(error)
            })?;
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        self.install(&mut tx, &command, &decision).await?;
        tx.commit().await.map_err(unavailable)?;
        Ok(decision)
    }

    async fn install(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        command: &OwnershipCommand,
        decision: &OwnershipDecision,
    ) -> Result<(), OwnershipError> {
        let row = sqlx::query("SELECT pending_operation_id,assignment_generation FROM case_ownership WHERE session_id=$1 FOR UPDATE")
            .bind(&command.session_id.0).fetch_one(&mut **tx).await.map_err(unavailable)?;
        let outcome: Option<serde_json::Value> =
            sqlx::query_scalar("SELECT outcome FROM assignment_operations WHERE request_id=$1")
                .bind(&command.request_id)
                .fetch_one(&mut **tx)
                .await
                .map_err(unavailable)?;
        if let Some(outcome) = outcome {
            if outcome != serde_json::to_value(decision).map_err(unavailable)? {
                return Err(unavailable(
                    "replayed coordinator decision differs from durable outcome",
                ));
            }
            return Ok(());
        }
        if row
            .get::<Option<String>, _>("pending_operation_id")
            .as_deref()
            != Some(&command.request_id)
            || u64::try_from(row.get::<i64, _>("assignment_generation")).map_err(unavailable)?
                != command.expected_generation
        {
            return Err(unavailable("pending ownership intent no longer matches"));
        }
        if let OwnershipDecision::Applied { ownership } = decision {
            let current: Option<String> =
                sqlx::query_scalar("SELECT volunteer_id FROM case_ownership WHERE session_id=$1")
                    .bind(&command.session_id.0)
                    .fetch_one(&mut **tx)
                    .await
                    .map_err(unavailable)?;
            let expected = AssignmentOwnership {
                volunteer_id: current,
                assignment_generation: command.expected_generation,
            }
            .decide(command);
            if &expected != decision {
                return Err(unavailable("coordinator generation diverges from PostgreSQL; offline reconciliation required"));
            }
            let snapshot: serde_json::Value = sqlx::query_scalar(
                "SELECT snapshot FROM session_checkpoints WHERE id=$1 FOR UPDATE",
            )
            .bind(&command.session_id.0)
            .fetch_one(&mut **tx)
            .await
            .map_err(unavailable)?;
            let mut session: Session = serde_json::from_value(snapshot).map_err(unavailable)?;
            match &command.transition {
                OwnershipTransition::Claim { volunteer_id } => {
                    session.assign_volunteer(volunteer_id)
                }
                OwnershipTransition::Transfer {
                    to_volunteer_id, ..
                } => session.transfer_handoff_to(to_volunteer_id),
                OwnershipTransition::Release { resolved, .. } => {
                    session.assigned_volunteer_id = None;
                    session.handoff_status = if *resolved {
                        HandoffStatus::Resolved
                    } else {
                        HandoffStatus::Pending
                    };
                    session.version = session
                        .version
                        .checked_add(1)
                        .ok_or_else(|| unavailable("session version exhausted"))?;
                    session.updated_at = Utc::now();
                }
            }
            session.assignment_generation = ownership.assignment_generation;
            let generation = i64::try_from(ownership.assignment_generation).map_err(unavailable)?;
            sqlx::query("UPDATE volunteer_assignments SET resolved_at=now(),updated_at=now(),status=$2 WHERE session_id=$1 AND resolved_at IS NULL")
                .bind(&command.session_id.0).bind(if matches!(command.transition, OwnershipTransition::Transfer { .. }) { "transferred" } else { "resolved" })
                .execute(&mut **tx).await.map_err(unavailable)?;
            if let Some(owner) = &ownership.volunteer_id {
                sqlx::query("INSERT INTO volunteer_assignments(session_id,volunteer_id,risk_level,status,assignment_generation) VALUES($1,$2,$3,'assigned',$4)")
                    .bind(&command.session_id.0).bind(owner).bind(i16::from(session.risk.level.0)).bind(generation)
                    .execute(&mut **tx).await.map_err(unavailable)?;
            }
            sqlx::query("UPDATE session_checkpoints SET snapshot=$2,version=$3,updated_at=$4,archived=false WHERE id=$1")
                .bind(&command.session_id.0).bind(serde_json::to_value(&session).map_err(unavailable)?)
                .bind(i64::try_from(session.version).map_err(unavailable)?).bind(session.updated_at)
                .execute(&mut **tx).await.map_err(unavailable)?;
            sqlx::query("UPDATE case_ownership SET volunteer_id=$2,assignment_generation=$3,updated_at=now() WHERE session_id=$1")
                .bind(&command.session_id.0).bind(&ownership.volunteer_id).bind(generation)
                .execute(&mut **tx).await.map_err(unavailable)?;
        }
        sqlx::query("UPDATE case_ownership SET pending_operation_id=NULL WHERE session_id=$1")
            .bind(&command.session_id.0)
            .execute(&mut **tx)
            .await
            .map_err(unavailable)?;
        sqlx::query(
            "UPDATE assignment_operations SET outcome=$2,completed_at=now() WHERE request_id=$1",
        )
        .bind(&command.request_id)
        .bind(serde_json::to_value(decision).map_err(unavailable)?)
        .execute(&mut **tx)
        .await
        .map_err(unavailable)?;
        Ok(())
    }

    pub async fn recover(&self) -> Result<(), OwnershipError> {
        let commands: Vec<serde_json::Value> = sqlx::query_scalar("SELECT command FROM assignment_operations WHERE outcome IS NULL ORDER BY created_at LIMIT 50")
            .fetch_all(&self.pool).await.map_err(unavailable)?;
        for value in commands {
            let command = serde_json::from_value(value).map_err(unavailable)?;
            self.execute(command).await?;
        }
        Ok(())
    }

    pub async fn add_note(
        &self,
        id: &str,
        volunteer: &str,
        generation: u64,
        request_id: &str,
        body: &str,
    ) -> Result<(), OwnershipError> {
        if generation == 0
            || generation > MAX_ASSIGNMENT_GENERATION
            || body.trim().is_empty()
            || body.len() > 4000
            || request_id.is_empty()
            || request_id.len() > 128
        {
            return Err(OwnershipError::Invalid(
                "positive generation, request_id and note of 1-4000 bytes required".into(),
            ));
        }
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        let row = sqlx::query("SELECT volunteer_id,assignment_generation,pending_operation_id FROM case_ownership WHERE session_id=$1 FOR UPDATE")
            .bind(id).fetch_optional(&mut *tx).await.map_err(unavailable)?.ok_or(OwnershipError::NotFound)?;
        if u64::try_from(row.get::<i64, _>("assignment_generation")).map_err(unavailable)?
            != generation
        {
            return Err(OwnershipError::Stale);
        }
        if row.get::<Option<String>, _>("volunteer_id").as_deref() != Some(volunteer) {
            return Err(OwnershipError::Conflict);
        }
        if row
            .get::<Option<String>, _>("pending_operation_id")
            .is_some()
        {
            return Err(OwnershipError::Pending);
        }
        let existing = sqlx::query("SELECT session_id,volunteer_id,assignment_generation,body FROM volunteer_case_notes WHERE request_id=$1")
            .bind(request_id).fetch_optional(&mut *tx).await.map_err(unavailable)?;
        if let Some(existing) = existing {
            if existing.get::<String, _>("session_id") != id
                || existing.get::<String, _>("volunteer_id") != volunteer
                || existing.get::<i64, _>("assignment_generation")
                    != i64::try_from(generation).map_err(unavailable)?
                || existing.get::<String, _>("body") != body
            {
                return Err(OwnershipError::Conflict);
            }
        } else {
            sqlx::query("INSERT INTO volunteer_case_notes(request_id,session_id,volunteer_id,assignment_generation,body) VALUES($1,$2,$3,$4,$5)")
                .bind(request_id).bind(id).bind(volunteer).bind(i64::try_from(generation).map_err(unavailable)?).bind(body)
                .execute(&mut *tx).await.map_err(unavailable)?;
        }
        tx.commit().await.map_err(unavailable)?;
        Ok(())
    }
}

fn validate_command(command: &OwnershipCommand) -> Result<(), OwnershipError> {
    let valid_name = |name: &str| !name.trim().is_empty() && name.len() <= 128;
    let valid_transition = match &command.transition {
        OwnershipTransition::Claim { volunteer_id }
        | OwnershipTransition::Release { volunteer_id, .. } => valid_name(volunteer_id),
        OwnershipTransition::Transfer {
            from_volunteer_id,
            to_volunteer_id,
        } => valid_name(from_volunteer_id) && valid_name(to_volunteer_id),
    };
    if !valid_name(&command.request_id)
        || !valid_name(&command.session_id.0)
        || !valid_transition
        || command.expected_generation > MAX_ASSIGNMENT_GENERATION
    {
        return Err(OwnershipError::Invalid(
            "invalid ownership identifiers or generation".into(),
        ));
    }
    Ok(())
}
