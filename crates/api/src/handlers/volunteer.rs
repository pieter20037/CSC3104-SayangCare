use crate::state::{AppState, Volunteer};
use actix_web::{web, HttpResponse, Responder};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Deserialize)]
pub struct ClaimRequest {
    pub volunteer_id: String,
    #[serde(default)]
    pub simulate: bool,
    pub session_id: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ClaimResponse {
    pub session_id: Option<String>,
    pub volunteer_id: Option<String>,
    pub handoff_status: String,
}

#[derive(Debug, Serialize)]
pub struct QueueItem {
    pub session_id: String,
    pub risk: u8,
    pub handoff_status: String,
    pub assigned_volunteer_id: Option<String>,
    pub state: String,
}

#[derive(Debug, Serialize)]
pub struct QueueResponse {
    pub pending: Vec<QueueItem>,
}

#[derive(Debug, Serialize)]
pub struct AssignedCaseItem {
    pub session_id: String,
    pub risk: u8,
    pub status: String,
}

#[derive(Debug, Serialize)]
pub struct VolunteerListResponse {
    pub volunteers: Vec<Volunteer>,
}

#[derive(Debug, Serialize)]
pub struct AssignedCasesResponse {
    pub volunteer_id: String,
    pub cases: Vec<AssignedCaseItem>,
}

#[derive(Debug, Serialize)]
pub struct ResolveCaseResponse {
    pub session_id: String,
    pub volunteer_id: String,
    pub handoff_status: String,
}

#[derive(Debug, Serialize)]
pub struct TransferCaseResponse {
    pub session_id: String,
    pub volunteer_id: String,
    pub handoff_status: String,
}

#[derive(Debug, Serialize)]
pub struct AlertsResponse {
    pub alerts: Vec<crate::state::OperatorAlert>,
}

pub async fn list_volunteers(state: web::Data<Arc<AppState>>) -> impl Responder {
    let volunteers = state
        .volunteers
        .read()
        .expect("volunteer registry lock poisoned")
        .values()
        .cloned()
        .collect::<Vec<_>>();
    HttpResponse::Ok().json(VolunteerListResponse { volunteers })
}

pub async fn list_pending(state: web::Data<Arc<AppState>>) -> impl Responder {
    match state.queue.list_pending().await {
        Ok(sids) => {
            let mut pending = Vec::new();
            for sid in sids {
                if let Ok(Some(session)) = state.sessions.get(&sid).await {
                    if session.state != sayangcare_core::domain::SessionState::Escalated
                        || session.handoff_status != sayangcare_core::domain::HandoffStatus::Pending
                    {
                        continue;
                    }
                    pending.push(QueueItem {
                        session_id: sid.0,
                        risk: session.risk.level.0,
                        handoff_status: format!("{:?}", session.handoff_status).to_lowercase(),
                        assigned_volunteer_id: session.assigned_volunteer_id,
                        state: format!("{:?}", session.state).to_lowercase(),
                    });
                }
            }
            HttpResponse::Ok().json(QueueResponse { pending })
        }
        Err(e) => {
            tracing::error!(error = %e, "queue listing failed");
            HttpResponse::InternalServerError().finish()
        }
    }
}

pub async fn list_assigned(
    state: web::Data<Arc<AppState>>,
    volunteer_id: web::Path<String>,
) -> impl Responder {
    let volunteer_id = volunteer_id.into_inner();
    let session_ids = match state
        .volunteer_bindings
        .list_active_for_volunteer(&volunteer_id)
        .await
    {
        Ok(session_ids) => session_ids
            .into_iter()
            .map(sayangcare_core::domain::SessionId)
            .collect::<Vec<_>>(),
        Err(error) => {
            tracing::error!(error = %error, volunteer_id = %volunteer_id, "assigned case listing failed");
            return HttpResponse::InternalServerError().finish();
        }
    };

    let mut cases = Vec::new();
    for session_id in session_ids {
        let sid = session_id.clone();
        if let Ok(Some(session)) = state.sessions.get(&sid).await {
            if session.assigned_volunteer_id.as_deref() != Some(volunteer_id.as_str())
                || !matches!(
                    session.handoff_status,
                    sayangcare_core::domain::HandoffStatus::Assigned
                        | sayangcare_core::domain::HandoffStatus::Transferred
                )
            {
                continue;
            }
            cases.push(AssignedCaseItem {
                session_id: session_id.0,
                risk: session.risk.level.0,
                status: format!("{:?}", session.handoff_status).to_lowercase(),
            });
        }
    }

    HttpResponse::Ok().json(AssignedCasesResponse {
        volunteer_id,
        cases,
    })
}

pub async fn claim(
    state: web::Data<Arc<AppState>>,
    body: web::Json<ClaimRequest>,
) -> impl Responder {
    let volunteer = state
        .volunteers
        .read()
        .expect("volunteer registry lock poisoned")
        .get(&body.volunteer_id)
        .cloned();
    let Some(volunteer) = volunteer.filter(|volunteer| volunteer.on_shift) else {
        return HttpResponse::Forbidden().json(serde_json::json!({
            "error": "volunteer is not registered or is off shift",
            "volunteer_id": body.volunteer_id,
        }));
    };

    if body.simulate
        && !body
            .session_id
            .as_deref()
            .is_some_and(|session_id| session_id.starts_with("SIM-"))
    {
        return HttpResponse::BadRequest()
            .body("simulation claims require a SIM- prefixed session_id");
    }

    let claim_result = if let Some(session_id) = body.session_id.as_deref() {
        let sid = sayangcare_core::domain::SessionId(session_id.to_string());
        match state.queue.claim_session(&sid, &body.volunteer_id).await {
            Ok(true) => Ok(Some(sid)),
            Ok(false) => Ok(None),
            Err(error) => Err(error),
        }
    } else {
        loop {
            match state.queue.claim(&body.volunteer_id).await {
                Ok(Some(sid)) => match state.sessions.get(&sid).await {
                    Ok(Some(session))
                        if session.state == sayangcare_core::domain::SessionState::Escalated
                            && session.handoff_status
                                == sayangcare_core::domain::HandoffStatus::Pending =>
                    {
                        break Ok(Some(sid));
                    }
                    Ok(Some(session)) => {
                        tracing::warn!(
                            session_id = %sid,
                            state = ?session.state,
                            handoff_status = ?session.handoff_status,
                            "discarding stale queue entry"
                        );
                        if let Err(error) =
                            state.queue.release_claim(&sid, &body.volunteer_id).await
                        {
                            tracing::error!(error = %error, session_id = %sid, "failed to release stale queue claim");
                        }
                    }
                    Ok(None) => {
                        tracing::warn!(session_id = %sid, "discarding queue entry without a session");
                        if let Err(error) =
                            state.queue.release_claim(&sid, &body.volunteer_id).await
                        {
                            tracing::error!(error = %error, session_id = %sid, "failed to release stale queue claim");
                        }
                    }
                    Err(error) => {
                        tracing::error!(error = %error, session_id = %sid, "session lookup failed while selecting next queue item");
                        if let Err(release_error) =
                            state.queue.release_claim(&sid, &body.volunteer_id).await
                        {
                            tracing::error!(error = %release_error, session_id = %sid, "failed to release queue claim after lookup failure");
                        }
                        break Err(error);
                    }
                },
                result => break result,
            }
        }
    };

    match claim_result {
        Ok(Some(sid)) => {
            let mut assigned_session = match state.sessions.get(&sid).await {
                Ok(Some(session)) => session,
                Ok(None) => {
                    let _ = state.queue.release_claim(&sid, &body.volunteer_id).await;
                    return HttpResponse::NotFound().json(serde_json::json!({
                        "error": "case session no longer exists",
                        "session_id": sid.0,
                    }));
                }
                Err(e) => {
                    tracing::error!(error = %e, "lookup failed during claim");
                    let _ = state.queue.release_claim(&sid, &body.volunteer_id).await;
                    let _ = state
                        .queue
                        .enqueue(sid.clone(), 4, chrono::Utc::now())
                        .await;
                    return HttpResponse::InternalServerError().finish();
                }
            };

            if assigned_session.state != sayangcare_core::domain::SessionState::Escalated
                || assigned_session.handoff_status
                    != sayangcare_core::domain::HandoffStatus::Pending
            {
                let _ = state.queue.release_claim(&sid, &body.volunteer_id).await;
                return HttpResponse::Conflict().json(serde_json::json!({
                    "error": "case is no longer pending or claimable",
                    "session_id": sid.0,
                    "state": format!("{:?}", assigned_session.state).to_lowercase(),
                    "handoff_status": format!("{:?}", assigned_session.handoff_status).to_lowercase(),
                }));
            }

            let binding_created = match state
                .volunteer_bindings
                .claim(&sid.0, &body.volunteer_id, assigned_session.risk.level.0)
                .await
            {
                Ok(true) => true,
                Ok(false) => {
                    let _ = state.queue.release_claim(&sid, &body.volunteer_id).await;
                    return HttpResponse::Conflict().json(serde_json::json!({
                        "error": "case already has a durable volunteer binding",
                        "session_id": sid.0,
                    }));
                }
                Err(error) => {
                    tracing::error!(error = %error, session_id = %sid, "durable binding claim failed");
                    let _ = state.queue.release_claim(&sid, &body.volunteer_id).await;
                    return HttpResponse::InternalServerError().finish();
                }
            };

            let expected = assigned_session.version;
            assigned_session.assign_volunteer(&body.volunteer_id);

            if let Err(error) = state.sessions.update_cas(&assigned_session, expected).await {
                tracing::warn!(error = %error, session_id = %sid, volunteer_id = %body.volunteer_id, "failed to persist volunteer assignment");
                if binding_created {
                    let _ = state
                        .volunteer_bindings
                        .release(&sid.0, &body.volunteer_id)
                        .await;
                }
                let _ = state.queue.release_claim(&sid, &body.volunteer_id).await;
                let _ = state
                    .queue
                    .enqueue(
                        sid.clone(),
                        assigned_session.risk.level.0,
                        chrono::Utc::now(),
                    )
                    .await;
                return HttpResponse::Conflict().json(serde_json::json!({
                    "error": "case assignment changed; refresh the queue and try again",
                    "session_id": sid.0,
                }));
            }

            if assigned_session.is_simulated || sid.0.starts_with("SIM-") || body.simulate {
                state
                    .record_alert(
                        "assigned",
                        &sid.0,
                        Some(&body.volunteer_id),
                        format!(
                        "Volunteer {} claimed simulated session {}; no live phone call was placed.",
                        body.volunteer_id, sid.0
                    ),
                    )
                    .await;
            } else {
                let dispatch_result = async {
                    let from_number = state
                        .config
                        .telephony
                        .twilio_from_number
                        .as_deref()
                        .filter(|number| !number.trim().is_empty())
                        .ok_or_else(|| sayangcare_core::CoreError::Telephony(
                            "SAYANGCARE__TELEPHONY__TWILIO_FROM_NUMBER is required for outbound volunteer calls".to_string(),
                        ))?;
                    let mut connect_url = reqwest::Url::parse(&format!(
                        "{}{}/outbound-connect",
                        state.config.telephony.public_base_url.trim_end_matches('/'),
                        "/twilio"
                    ))
                    .map_err(|error| sayangcare_core::CoreError::Telephony(format!(
                        "invalid public URL for outbound TwiML: {error}"
                    )))?;
                    connect_url
                        .query_pairs_mut()
                        .append_pair("session_id", &sid.0);

                    let outbound_sid = state
                        .telephony
                        .create_outbound_call(
                            &volunteer.phone_number,
                            from_number,
                            connect_url.as_str(),
                        )
                        .await?;
                    let conference = crate::handlers::twilio::conference_name(&sid.0);
                    if let Err(error) = state
                        .telephony
                        .move_call_to_conference(&sid.0, &conference)
                        .await
                    {
                        if let Err(hangup_error) = state.telephony.hangup(&outbound_sid).await {
                            tracing::warn!(error = %hangup_error, outbound_call_sid = %outbound_sid, "failed to cancel volunteer call after caller conference setup failed");
                        }
                        return Err(error);
                    }
                    Ok::<String, sayangcare_core::CoreError>(outbound_sid)
                }
                .await;

                match dispatch_result {
                    Ok(outbound_sid) => {
                        state.record_alert(
                            "assigned",
                            &sid.0,
                            Some(&body.volunteer_id),
                            format!("Volunteer {} claimed session {} and outbound dispatch {} was started.", body.volunteer_id, sid.0, outbound_sid),
                        ).await;
                    }
                    Err(error) => {
                        tracing::warn!(error = %error, session_id = %sid, volunteer_id = %body.volunteer_id, "outbound volunteer dispatch failed");
                        match state
                            .telephony
                            .redirect_to_human(&sid.0, &body.volunteer_id)
                            .await
                        {
                            Ok(()) => state.record_alert(
                                "handoff_fallback",
                                &sid.0,
                                Some(&body.volunteer_id),
                                format!("Outbound dispatch failed for session {}; the existing-call TwiML handoff was attempted instead.", sid.0),
                            ).await,
                            Err(fallback_error) => {
                                tracing::warn!(error = %fallback_error, session_id = %sid, volunteer_id = %body.volunteer_id, "existing-call TwiML handoff fallback failed");
                                state.record_alert(
                                    "handoff_failed",
                                    &sid.0,
                                    Some(&body.volunteer_id),
                                    format!("Outbound dispatch for session {} failed; fallback operator review is required.", sid.0),
                                ).await;
                            }
                        }
                    }
                }
            }

            HttpResponse::Ok().json(ClaimResponse {
                session_id: Some(sid.0),
                volunteer_id: Some(body.volunteer_id.clone()),
                handoff_status: "assigned".to_string(),
            })
        }
        Ok(None) => HttpResponse::Ok().json(ClaimResponse {
            session_id: None,
            volunteer_id: None,
            handoff_status: "waiting".to_string(),
        }),
        Err(e) => {
            tracing::error!(error = %e, "claim failed");
            HttpResponse::InternalServerError().finish()
        }
    }
}

pub async fn resolve_case(
    state: web::Data<Arc<AppState>>,
    path: web::Path<(String, String)>,
) -> impl Responder {
    let (volunteer_id, session_id) = path.into_inner();
    let sid = sayangcare_core::domain::SessionId(session_id.clone());

    let mut session = match state.sessions.get(&sid).await {
        Ok(Some(session)) => session,
        Ok(None) => return HttpResponse::NotFound().finish(),
        Err(e) => {
            tracing::error!(error = %e, session_id = %sid, volunteer_id = %volunteer_id, "lookup failed while resolving case");
            return HttpResponse::InternalServerError().finish();
        }
    };

    if session.assigned_volunteer_id.as_deref() != Some(volunteer_id.as_str()) {
        return HttpResponse::Conflict().json(serde_json::json!({
            "error": "case is not assigned to this volunteer",
            "session_id": session_id,
            "volunteer_id": volunteer_id,
        }));
    }

    let expected = session.version;
    session.resolve_handoff();

    if let Err(e) = state.sessions.update_cas(&session, expected).await {
        tracing::warn!(error = %e, session_id = %sid, volunteer_id = %volunteer_id, "failed to persist resolved handoff");
        return HttpResponse::Conflict().finish();
    }

    if let Err(error) = state
        .volunteer_bindings
        .release(&session_id, &volunteer_id)
        .await
    {
        tracing::warn!(error = %error, session_id = %sid, volunteer_id = %volunteer_id, "failed to resolve durable volunteer binding");
        return HttpResponse::InternalServerError().finish();
    }

    if let Err(error) = state.queue.release_claim(&sid, &volunteer_id).await {
        tracing::warn!(error = %error, session_id = %sid, volunteer_id = %volunteer_id, "failed to clear resolved queue claim");
    }

    state
        .record_alert(
            "resolved",
            &session_id,
            Some(&volunteer_id),
            format!(
                "Volunteer {} resolved case {} after live handoff review.",
                volunteer_id, session_id
            ),
        )
        .await;

    HttpResponse::Ok().json(ResolveCaseResponse {
        session_id: session_id.clone(),
        volunteer_id: volunteer_id.clone(),
        handoff_status: "resolved".to_string(),
    })
}

pub async fn list_alerts(state: web::Data<Arc<AppState>>) -> impl Responder {
    match state.list_alerts().await {
        Ok(alerts) => HttpResponse::Ok().json(AlertsResponse { alerts }),
        Err(error) => {
            tracing::error!(error = %error, "failed to list operator alerts");
            HttpResponse::ServiceUnavailable().finish()
        }
    }
}

pub async fn transfer_case(
    state: web::Data<Arc<AppState>>,
    path: web::Path<(String, String)>,
) -> impl Responder {
    let (volunteer_id, session_id) = path.into_inner();
    let sid = sayangcare_core::domain::SessionId(session_id.clone());

    let mut session = match state.sessions.get(&sid).await {
        Ok(Some(session)) => session,
        Ok(None) => return HttpResponse::NotFound().finish(),
        Err(e) => {
            tracing::error!(error = %e, session_id = %sid, volunteer_id = %volunteer_id, "lookup failed while transferring case");
            return HttpResponse::InternalServerError().finish();
        }
    };

    if session.assigned_volunteer_id.as_deref() != Some(volunteer_id.as_str()) {
        return HttpResponse::Conflict().json(serde_json::json!({
            "error": "case is not assigned to this volunteer",
            "session_id": session_id,
            "volunteer_id": volunteer_id,
        }));
    }

    let next_volunteer = {
        let volunteers = state
            .volunteers
            .read()
            .expect("volunteer registry lock poisoned");
        let mut available = volunteers
            .values()
            .filter(|volunteer| volunteer.on_shift && volunteer.id != volunteer_id)
            .cloned()
            .collect::<Vec<_>>();
        available.sort_by(|left, right| left.id.cmp(&right.id));
        available.into_iter().next()
    };
    let Some(next_volunteer) = next_volunteer else {
        return HttpResponse::Conflict().json(serde_json::json!({
            "error": "no other on-shift volunteer is available",
            "session_id": session_id,
            "volunteer_id": volunteer_id,
        }));
    };

    if !session.is_simulated && !session_id.starts_with("SIM-") {
        if let Err(error) = state
            .telephony
            .redirect_to_human(&session_id, &next_volunteer.id)
            .await
        {
            tracing::warn!(error = %error, session_id = %sid, from_volunteer = %volunteer_id, to_volunteer = %next_volunteer.id, "live call transfer failed");
            state
                .record_alert(
                    "transfer_failed",
                    &session_id,
                    Some(&volunteer_id),
                    format!(
                        "Transfer of live session {} to {} failed: {}",
                        session_id, next_volunteer.id, error
                    ),
                )
                .await;
            return HttpResponse::BadGateway().json(serde_json::json!({
                "error": "live call redirect failed",
                "detail": error.to_string(),
                "session_id": session_id,
                "from_volunteer_id": volunteer_id,
                "to_volunteer_id": next_volunteer.id,
            }));
        }
    }

    let expected = session.version;
    session.transfer_handoff_to(next_volunteer.id.clone());

    match state
        .queue
        .move_claim(&sid, &volunteer_id, &next_volunteer.id)
        .await
    {
        Ok(true) => {}
        Ok(false) => {
            return HttpResponse::Conflict().json(serde_json::json!({
                "error": "case assignment changed; refresh and try again",
                "session_id": session_id,
            }));
        }
        Err(error) => {
            tracing::error!(error = %error, session_id = %sid, "failed to transfer queue claim");
            return HttpResponse::InternalServerError().finish();
        }
    }

    let binding_transferred = match state
        .volunteer_bindings
        .transfer(&session_id, &volunteer_id, &next_volunteer.id)
        .await
    {
        Ok(transferred) => transferred,
        Err(error) => {
            tracing::error!(error = %error, session_id = %sid, "failed to transfer durable volunteer binding");
            let _ = state
                .queue
                .move_claim(&sid, &next_volunteer.id, &volunteer_id)
                .await;
            return HttpResponse::InternalServerError().finish();
        }
    };
    if !binding_transferred {
        let _ = state
            .queue
            .move_claim(&sid, &next_volunteer.id, &volunteer_id)
            .await;
        return HttpResponse::Conflict().json(serde_json::json!({
            "error": "durable volunteer binding changed; refresh and try again",
            "session_id": session_id,
        }));
    }

    if let Err(e) = state.sessions.update_cas(&session, expected).await {
        tracing::warn!(error = %e, session_id = %sid, volunteer_id = %volunteer_id, "failed to persist transferred handoff");
        let _ = state
            .queue
            .move_claim(&sid, &next_volunteer.id, &volunteer_id)
            .await;
        let _ = state
            .volunteer_bindings
            .transfer(&session_id, &next_volunteer.id, &volunteer_id)
            .await;
        return HttpResponse::Conflict().finish();
    }

    state
        .record_alert(
            "transferred",
            &session_id,
            Some(&next_volunteer.id),
            format!(
                "Volunteer {} transferred session {} to volunteer {}.",
                volunteer_id, session_id, next_volunteer.id
            ),
        )
        .await;

    HttpResponse::Ok().json(TransferCaseResponse {
        session_id: session_id.clone(),
        volunteer_id: next_volunteer.id,
        handoff_status: "transferred".to_string(),
    })
}
