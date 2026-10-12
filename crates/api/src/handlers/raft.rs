use crate::state::AppState;
use actix_web::{web, HttpRequest, HttpResponse};
use openraft::error::{InstallSnapshotError, RaftError};
use openraft::raft::{
    AppendEntriesRequest, AppendEntriesResponse, InstallSnapshotRequest, InstallSnapshotResponse,
    VoteRequest, VoteResponse,
};
use sayangcare_priority_queue::{
    RaftQueueCommand, RaftQueueNode, RaftQueueTypeConfig, WriteReply, TOKEN_HEADER,
};
use std::sync::Arc;

type State = web::Data<Arc<AppState>>;

/// Returns the local Raft node when enabled and the peer token (if configured) matches.
fn authorize(state: &AppState, req: &HttpRequest) -> Result<Arc<RaftQueueNode>, HttpResponse> {
    let Some(node) = state.raft.clone() else {
        return Err(HttpResponse::NotFound().finish());
    };
    if let Some(expected) = node.token() {
        let provided = req
            .headers()
            .get(TOKEN_HEADER)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default();
        let matches = provided.len() == expected.len()
            && provided
                .bytes()
                .zip(expected.bytes())
                .fold(0u8, |diff, (a, b)| diff | (a ^ b))
                == 0;
        if !matches {
            return Err(HttpResponse::Unauthorized().finish());
        }
    }
    Ok(node)
}

pub async fn append(
    state: State,
    req: HttpRequest,
    body: web::Json<AppendEntriesRequest<RaftQueueTypeConfig>>,
) -> HttpResponse {
    match authorize(&state, &req) {
        Ok(node) => {
            let result: Result<AppendEntriesResponse<u64>, RaftError<u64>> =
                node.raft().append_entries(body.into_inner()).await;
            HttpResponse::Ok().json(result)
        }
        Err(response) => response,
    }
}

pub async fn vote(
    state: State,
    req: HttpRequest,
    body: web::Json<VoteRequest<u64>>,
) -> HttpResponse {
    match authorize(&state, &req) {
        Ok(node) => {
            let result: Result<VoteResponse<u64>, RaftError<u64>> =
                node.raft().vote(body.into_inner()).await;
            HttpResponse::Ok().json(result)
        }
        Err(response) => response,
    }
}

pub async fn snapshot(
    state: State,
    req: HttpRequest,
    body: web::Json<InstallSnapshotRequest<RaftQueueTypeConfig>>,
) -> HttpResponse {
    match authorize(&state, &req) {
        Ok(node) => {
            let result: Result<InstallSnapshotResponse<u64>, RaftError<u64, InstallSnapshotError>> =
                node.raft().install_snapshot(body.into_inner()).await;
            HttpResponse::Ok().json(result)
        }
        Err(response) => response,
    }
}

/// Leader-side endpoint that followers forward queue mutations to.
pub async fn write(
    state: State,
    req: HttpRequest,
    body: web::Json<RaftQueueCommand>,
) -> HttpResponse {
    match authorize(&state, &req) {
        Ok(node) => {
            let result: Result<WriteReply, String> = node.write_local(body.into_inner()).await;
            HttpResponse::Ok().json(result)
        }
        Err(response) => response,
    }
}

/// Public, read-only view of this node's role in the queue cluster.
pub async fn status(state: State) -> HttpResponse {
    let Some(node) = state.raft.as_ref() else {
        return HttpResponse::Ok().json(serde_json::json!({ "backend": "redis" }));
    };
    let metrics = node.raft().metrics().borrow().clone();
    HttpResponse::Ok().json(serde_json::json!({
        "backend": "raft",
        "node_id": metrics.id,
        "state": format!("{:?}", metrics.state),
        "current_term": metrics.current_term,
        "current_leader": metrics.current_leader,
        "last_log_index": metrics.last_log_index,
        "last_applied": metrics.last_applied.map(|id| id.index),
    }))
}
