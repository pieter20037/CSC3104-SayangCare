use crate::state::{AppState, Volunteer};
use actix_web::{web, HttpResponse, Responder};
use sayangcare_core::domain::{HandoffStatus, Session, SessionId, SessionState};
use sayangcare_core::ownership::{OwnershipCommand, OwnershipDecision, OwnershipTransition};
use sayangcare_session_store::OwnershipError;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

#[cfg(test)]
#[path = "volunteer_tests.rs"]
mod tests;

type State = web::Data<Arc<AppState>>;

#[derive(Debug, Deserialize)]
pub struct ClaimRequest {
    pub volunteer_id: String,
    #[serde(default)]
    pub simulate: bool,
    pub session_id: Option<String>,
    pub request_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CaseAction {
    pub assignment_generation: u64,
    pub request_id: String,
    pub to_volunteer_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct NoteRequest {
    pub assignment_generation: u64,
    pub request_id: String,
    pub body: String,
}

#[derive(Debug, Serialize)]
pub struct QueueItem {
    pub session_id: String,
    pub risk: u8,
    pub handoff_status: String,
    pub assigned_volunteer_id: Option<String>,
    pub state: String,
}

pub fn ownership_error(error: OwnershipError) -> HttpResponse {
    tracing::warn!(%error, "volunteer action rejected");
    let mut response = match error {
        OwnershipError::Stale | OwnershipError::Conflict => HttpResponse::Conflict(),
        OwnershipError::Pending | OwnershipError::Unavailable(_) => {
            HttpResponse::ServiceUnavailable()
        }
        OwnershipError::NotFound => HttpResponse::NotFound(),
        OwnershipError::Invalid(_) => HttpResponse::BadRequest(),
    };
    response.json(serde_json::json!({"error": error.to_string()}))
}

fn decision_error(decision: &OwnershipDecision) -> Option<HttpResponse> {
    match decision {
        OwnershipDecision::Applied { .. } => None,
        OwnershipDecision::StaleGeneration => Some(ownership_error(OwnershipError::Stale)),
        OwnershipDecision::Conflict | OwnershipDecision::RequestIdConflict => {
            Some(ownership_error(OwnershipError::Conflict))
        }
        OwnershipDecision::GenerationExhausted => Some(ownership_error(OwnershipError::Invalid(
            "assignment generation exhausted".into(),
        ))),
    }
}

fn on_shift(state: &AppState, id: &str) -> Option<Volunteer> {
    state
        .volunteers
        .read()
        .expect("volunteer registry lock poisoned")
        .get(id)
        .filter(|volunteer| volunteer.on_shift)
        .cloned()
}

pub async fn list_volunteers(state: State) -> impl Responder {
    let volunteers: Vec<_> = state
        .volunteers
        .read()
        .expect("volunteer registry lock poisoned")
        .values()
        .cloned()
        .collect();
    HttpResponse::Ok().json(serde_json::json!({"volunteers": volunteers}))
}

pub async fn list_pending(state: State) -> impl Responder {
    let ids = match state.queue.list_pending().await {
        Ok(ids) => ids,
        Err(error) => return ownership_error(OwnershipError::Unavailable(error.to_string())),
    };
    let mut pending = Vec::new();
    for sid in ids {
        match state.sessions.get(&sid).await {
            Ok(Some(session))
                if session.state == SessionState::Escalated
                    && session.handoff_status == HandoffStatus::Pending =>
            {
                pending.push(QueueItem {
                    session_id: sid.0,
                    risk: session.risk.level.0,
                    handoff_status: "pending".into(),
                    assigned_volunteer_id: session.assigned_volunteer_id,
                    state: "escalated".into(),
                });
            }
            Ok(_) => {}
            Err(error) => return ownership_error(OwnershipError::Unavailable(error.to_string())),
        }
    }
    HttpResponse::Ok().json(serde_json::json!({"pending": pending}))
}

pub async fn list_assigned(state: State, volunteer_id: web::Path<String>) -> impl Responder {
    let id = volunteer_id.into_inner();
    let ids = match state
        .volunteer_bindings
        .list_active_for_volunteer(&id)
        .await
    {
        Ok(ids) => ids,
        Err(error) => return ownership_error(OwnershipError::Unavailable(error.to_string())),
    };
    let mut cases = Vec::new();
    for session_id in ids {
        let view = match state.ownership.current(&session_id).await {
            Ok(view) => view,
            Err(error) => return ownership_error(error),
        };
        match state.sessions.get(&SessionId(session_id.clone())).await {
            Ok(Some(session)) if view.volunteer_id.as_deref() == Some(&id) => {
                cases.push(serde_json::json!({
                    "session_id": session_id, "risk":session.risk.level.0,
                    "status": format!("{:?}", session.handoff_status).to_lowercase(),
                    "assignment_generation":view.assignment_generation,
                    "coordinator":view.coordinator,
                    "pending_operation_id":view.pending_operation_id
                }));
            }
            Ok(_) => {}
            Err(error) => return ownership_error(OwnershipError::Unavailable(error.to_string())),
        }
    }
    HttpResponse::Ok().json(serde_json::json!({"volunteer_id":id,"cases":cases}))
}

pub async fn ownership(state: State, sid: web::Path<String>) -> impl Responder {
    match state.ownership.current(&sid).await {
        Ok(view) => HttpResponse::Ok().json(view),
        Err(error) => ownership_error(error),
    }
}

pub async fn ownership_audit(state: State, sid: web::Path<String>) -> impl Responder {
    let audit: Result<(serde_json::Value,serde_json::Value),sqlx::Error> = async {
        let history: serde_json::Value = sqlx::query_scalar(
            "SELECT COALESCE(jsonb_agg(jsonb_build_object('volunteer_id',volunteer_id,'assignment_generation',assignment_generation,'status',status,'active',resolved_at IS NULL) ORDER BY assignment_id),'[]'::jsonb) FROM volunteer_assignments WHERE session_id=$1",
        ).bind(sid.as_str()).fetch_one(&state.postgres).await?;
        let notes: serde_json::Value = sqlx::query_scalar(
            "SELECT COALESCE(jsonb_agg(jsonb_build_object('request_id',request_id,'volunteer_id',volunteer_id,'assignment_generation',assignment_generation) ORDER BY note_id),'[]'::jsonb) FROM volunteer_case_notes WHERE session_id=$1",
        ).bind(sid.as_str()).fetch_one(&state.postgres).await?;
        Ok((history,notes))
    }.await;
    match audit {
        Ok((history, notes)) => HttpResponse::Ok()
            .json(serde_json::json!({"session_id":sid.as_str(),"history":history,"notes":notes})),
        Err(error) => ownership_error(OwnershipError::Unavailable(error.to_string())),
    }
}

pub async fn claim(state: State, body: web::Json<ClaimRequest>) -> impl Responder {
    let Some(volunteer) = on_shift(&state, &body.volunteer_id) else {
        return HttpResponse::Forbidden()
            .json(serde_json::json!({"error":"volunteer is not registered or is off shift"}));
    };
    if body.simulate
        && !body
            .session_id
            .as_deref()
            .is_some_and(|id| id.starts_with("SIM-"))
    {
        return HttpResponse::BadRequest().json(
            serde_json::json!({"error":"simulation claims require a SIM- prefixed session_id"}),
        );
    }
    let request_id = body
        .request_id
        .clone()
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    let prior: Option<String> = match sqlx::query_scalar(
        "SELECT session_id FROM assignment_operations WHERE request_id=$1",
    )
    .bind(&request_id)
    .fetch_optional(&state.postgres)
    .await
    {
        Ok(prior) => prior,
        Err(error) => return ownership_error(OwnershipError::Unavailable(error.to_string())),
    };
    // Queue selection remains Redis risk/FIFO; durable ownership arbitrates claims.
    for _ in 0..100 {
        let sid = match body.session_id.as_ref().or(prior.as_ref()) {
            Some(id) => SessionId(id.clone()),
            None => match state.queue.peek_highest().await {
                Ok(Some(sid)) => sid,
                Ok(None) => {
                    return HttpResponse::Ok().json(serde_json::json!({
                        "session_id":null,"volunteer_id":null,"handoff_status":"waiting",
                        "assignment_generation":null,"request_id":request_id
                    }))
                }
                Err(error) => {
                    return ownership_error(OwnershipError::Unavailable(error.to_string()))
                }
            },
        };
        let view = match state.ownership.current(&sid.0).await {
            Ok(view) => view,
            Err(error) => return ownership_error(error),
        };
        let session = match state.sessions.get(&sid).await {
            Ok(session) => session,
            Err(error) => return ownership_error(OwnershipError::Unavailable(error.to_string())),
        };
        let eligible = session.as_ref().is_some_and(|s| {
            s.state == SessionState::Escalated && s.handoff_status == HandoffStatus::Pending
        });
        // A request-id replay must reach the operation journal even after assignment.
        if !eligible && body.session_id.is_none() && prior.is_none() {
            if let Err(error) = discard_stale(&state, &sid, &body.volunteer_id).await {
                return ownership_error(OwnershipError::Unavailable(error.to_string()));
            }
            continue;
        }
        let command = OwnershipCommand {
            request_id: request_id.clone(),
            session_id: sid.clone(),
            expected_generation: view.assignment_generation,
            transition: OwnershipTransition::Claim {
                volunteer_id: body.volunteer_id.clone(),
            },
        };
        // Recover the immutable original payload when retrying an acknowledged or uncertain claim.
        let (command, replay) = match original_command(&state, command).await {
            Ok(command) => command,
            Err(error) => return ownership_error(error),
        };
        let coordinator = if view.assignment_generation > 0 || view.pending_operation_id.is_some() {
            view.coordinator.as_str()
        } else {
            state.ownership.mode()
        };
        let decision = match state.ownership.execute(command).await {
            Ok(result) => result,
            Err(error) => return ownership_error(error),
        };
        if let Some(error) = decision_error(&decision) {
            return error;
        }
        let OwnershipDecision::Applied { ownership } = decision else {
            unreachable!()
        };
        if let Err(error) = state.reconcile_ownership_projections().await {
            tracing::error!(%error, session_id=%sid, "assignment committed; projection repair remains pending");
        }
        if !replay {
            if let Some(session) = session {
                dispatch_claim(&state, &session, &volunteer).await;
            }
        }
        return HttpResponse::Ok().json(serde_json::json!({
            "session_id":sid.0, "volunteer_id":ownership.volunteer_id,
            "handoff_status":"assigned","assignment_generation":ownership.assignment_generation,
            "request_id":request_id,"coordinator":coordinator
        }));
    }
    ownership_error(OwnershipError::Unavailable(
        "too many stale queue entries; retry after reconciliation".into(),
    ))
}

async fn original_command(
    state: &AppState,
    command: OwnershipCommand,
) -> Result<(OwnershipCommand, bool), OwnershipError> {
    let original: Option<serde_json::Value> =
        sqlx::query_scalar("SELECT command FROM assignment_operations WHERE request_id=$1")
            .bind(&command.request_id)
            .fetch_optional(&state.postgres)
            .await
            .map_err(|e| OwnershipError::Unavailable(e.to_string()))?;
    if let Some(original) = original {
        let original: OwnershipCommand = serde_json::from_value(original)
            .map_err(|e| OwnershipError::Unavailable(e.to_string()))?;
        if original.session_id != command.session_id || original.transition != command.transition {
            return Err(OwnershipError::Conflict);
        }
        Ok((original, true))
    } else {
        Ok((command, false))
    }
}

async fn discard_stale(
    state: &AppState,
    sid: &SessionId,
    volunteer: &str,
) -> sayangcare_core::CoreResult<()> {
    if state.queue.claim_session(sid, volunteer).await? {
        state.queue.release_claim(sid, volunteer).await?;
    }
    Ok(())
}

pub async fn transfer_case(
    state: State,
    path: web::Path<(String, String)>,
    body: web::Json<CaseAction>,
) -> impl Responder {
    let (from, id) = path.into_inner();
    let to = match &body.to_volunteer_id {
        Some(to) => to.clone(),
        None => {
            let volunteers = state
                .volunteers
                .read()
                .expect("volunteer registry lock poisoned");
            let mut ids: Vec<_> = volunteers
                .values()
                .filter(|v| v.on_shift && v.id != from)
                .map(|v| v.id.clone())
                .collect();
            ids.sort();
            let Some(to) = ids.into_iter().next() else {
                return ownership_error(OwnershipError::Conflict);
            };
            to
        }
    };
    if on_shift(&state, &to).is_none() || to == from {
        return ownership_error(OwnershipError::Invalid(
            "target must be a different on-shift volunteer".into(),
        ));
    }
    action(
        &state,
        id,
        body.request_id.clone(),
        body.assignment_generation,
        OwnershipTransition::Transfer {
            from_volunteer_id: from,
            to_volunteer_id: to,
        },
        "transferred",
    )
    .await
}

pub async fn resolve_case(
    state: State,
    path: web::Path<(String, String)>,
    body: web::Json<CaseAction>,
) -> impl Responder {
    let (volunteer_id, id) = path.into_inner();
    action(
        &state,
        id,
        body.request_id.clone(),
        body.assignment_generation,
        OwnershipTransition::Release {
            volunteer_id,
            resolved: true,
        },
        "resolved",
    )
    .await
}

pub async fn release_case(
    state: State,
    path: web::Path<(String, String)>,
    body: web::Json<CaseAction>,
) -> impl Responder {
    let (volunteer_id, id) = path.into_inner();
    action(
        &state,
        id,
        body.request_id.clone(),
        body.assignment_generation,
        OwnershipTransition::Release {
            volunteer_id,
            resolved: false,
        },
        "pending",
    )
    .await
}

async fn action(
    state: &AppState,
    id: String,
    request_id: String,
    generation: u64,
    transition: OwnershipTransition,
    status: &str,
) -> HttpResponse {
    if generation == 0 {
        return ownership_error(OwnershipError::Invalid(
            "positive assignment_generation required".into(),
        ));
    }
    let coordinator = match state.ownership.current(&id).await {
        Ok(view) => view.coordinator,
        Err(error) => return ownership_error(error),
    };
    let decision = match state
        .ownership
        .execute(OwnershipCommand {
            request_id: request_id.clone(),
            session_id: SessionId(id.clone()),
            expected_generation: generation,
            transition: transition.clone(),
        })
        .await
    {
        Ok(result) => result,
        Err(error) => return ownership_error(error),
    };
    if let Some(error) = decision_error(&decision) {
        return error;
    }
    let OwnershipDecision::Applied { ownership } = decision else {
        unreachable!()
    };
    if let Err(error) = state.reconcile_ownership_projections().await {
        tracing::error!(%error, session_id=%id, "ownership committed; projection repair pending");
    }
    if let OwnershipTransition::Transfer {
        to_volunteer_id, ..
    } = transition
    {
        match state.sessions.get(&SessionId(id.clone())).await {
            Ok(Some(session)) if !session.is_simulated && !id.starts_with("SIM-") => {
                if let Err(error) = state
                    .telephony
                    .redirect_to_human(&id, &to_volunteer_id)
                    .await
                {
                    tracing::error!(%error, session_id=%id, "ownership transferred but live redirect failed");
                    state
                        .record_alert(
                            "transfer_failed",
                            &id,
                            Some(&to_volunteer_id),
                            error.to_string(),
                        )
                        .await;
                    return HttpResponse::BadGateway().json(serde_json::json!({
                        "error":"ownership committed but live redirect failed", "assignment_generation":ownership.assignment_generation,
                        "request_id":request_id,"session_id":id
                    }));
                }
            }
            Ok(_) => {}
            Err(error) => return ownership_error(OwnershipError::Unavailable(error.to_string())),
        }
    }
    state
        .record_alert(
            status,
            &id,
            ownership.volunteer_id.as_deref(),
            format!(
                "Ownership {status}; generation {}",
                ownership.assignment_generation
            ),
        )
        .await;
    HttpResponse::Ok().json(serde_json::json!({
        "session_id":id,"volunteer_id":ownership.volunteer_id,"assignment_generation":ownership.assignment_generation,
        "handoff_status":status,"request_id":request_id,"coordinator":coordinator
    }))
}

pub async fn add_note(
    state: State,
    path: web::Path<(String, String)>,
    body: web::Json<NoteRequest>,
) -> impl Responder {
    let (volunteer, id) = path.into_inner();
    match state.ownership.add_note(&id,&volunteer,body.assignment_generation,&body.request_id,&body.body).await {
        Ok(()) => HttpResponse::Ok().json(serde_json::json!({"session_id":id,"assignment_generation":body.assignment_generation,"request_id":body.request_id})),
        Err(error) => ownership_error(error),
    }
}

pub async fn list_alerts(state: State) -> impl Responder {
    match state.list_alerts().await {
        Ok(alerts) => HttpResponse::Ok().json(serde_json::json!({"alerts":alerts})),
        Err(error) => ownership_error(OwnershipError::Unavailable(error.to_string())),
    }
}

async fn dispatch_claim(state: &AppState, session: &Session, volunteer: &Volunteer) {
    let sid = &session.id.0;
    if session.is_simulated || sid.starts_with("SIM-") {
        state
            .record_alert(
                "assigned",
                sid,
                Some(&volunteer.id),
                format!(
                    "Volunteer {} claimed simulated session {}; no live phone call was placed.",
                    volunteer.id, sid
                ),
            )
            .await;
        return;
    }
    let dispatch = async {
        let from = state
            .config
            .telephony
            .twilio_from_number
            .as_deref()
            .filter(|v| !v.trim().is_empty())
            .ok_or_else(|| {
                sayangcare_core::CoreError::Telephony("TWILIO_FROM_NUMBER required".into())
            })?;
        let mut url = reqwest::Url::parse(&format!(
            "{}/twilio/outbound-connect",
            state.config.telephony.public_base_url.trim_end_matches('/')
        ))
        .map_err(|e| sayangcare_core::CoreError::Telephony(e.to_string()))?;
        url.query_pairs_mut().append_pair("session_id", sid);
        let outbound = state
            .telephony
            .create_outbound_call(&volunteer.phone_number, from, url.as_str())
            .await?;
        let conference = crate::handlers::twilio::conference_name(sid);
        if let Err(error) = state
            .telephony
            .move_call_to_conference(sid, &conference)
            .await
        {
            if let Err(cancel) = state.telephony.hangup(&outbound).await {
                tracing::error!(%cancel,"failed to cancel outbound dispatch");
            }
            return Err(error);
        }
        Ok::<_, sayangcare_core::CoreError>(outbound)
    }
    .await;
    match dispatch {
        Ok(outbound) => {
            state
                .record_alert(
                    "assigned",
                    sid,
                    Some(&volunteer.id),
                    format!("Outbound dispatch {outbound} started"),
                )
                .await
        }
        Err(error) => {
            tracing::error!(%error,session_id=%sid,"outbound dispatch failed; attempting existing-call handoff");
            match state.telephony.redirect_to_human(sid, &volunteer.id).await {
                Ok(()) => {
                    state
                        .record_alert(
                            "handoff_fallback",
                            sid,
                            Some(&volunteer.id),
                            error.to_string(),
                        )
                        .await
                }
                Err(fallback) => {
                    tracing::error!(%fallback,session_id=%sid,"fallback handoff failed");
                    state
                        .record_alert(
                            "handoff_failed",
                            sid,
                            Some(&volunteer.id),
                            fallback.to_string(),
                        )
                        .await;
                }
            }
        }
    }
}
