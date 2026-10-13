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
    HttpResponse::Ok().json(local_status(node))
}

fn local_status(node: &RaftQueueNode) -> serde_json::Value {
    let metrics = node.raft().metrics().borrow().clone();
    serde_json::json!({
        "backend": "raft",
        "node_id": metrics.id,
        "state": format!("{:?}", metrics.state),
        "current_term": metrics.current_term,
        "current_leader": metrics.current_leader,
        "last_log_index": metrics.last_log_index,
        "last_applied": metrics.last_applied.map(|id| id.index),
    })
}

/// Read-only cluster view for the dashboard: this node's status plus every peer's, so a
/// single page shows leader election and log replication across the whole cluster.
pub async fn cluster(state: State) -> HttpResponse {
    let Some(node) = state.raft.clone() else {
        return HttpResponse::Ok().json(serde_json::json!({ "backend": "redis" }));
    };
    let local = local_status(&node);
    let this_node = local["node_id"].as_u64().unwrap_or_default();

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_millis(1500))
        .build()
        .unwrap_or_default();

    let probes = node.peers().iter().map(|(id, url)| {
        let client = client.clone();
        let local = local.clone();
        let (id, url) = (*id, url.clone());
        async move {
            let status = if id == this_node {
                Some(local)
            } else {
                match client.get(format!("{url}/api/v1/queue/raft")).send().await {
                    Ok(resp) => resp.json::<serde_json::Value>().await.ok(),
                    Err(_) => None,
                }
            };
            serde_json::json!({
                "node_id": id,
                "url": url,
                "reachable": status.is_some(),
                "status": status,
            })
        }
    });
    let mut nodes = futures::future::join_all(probes).await;
    if nodes.is_empty() {
        nodes.push(serde_json::json!({
            "node_id": this_node,
            "url": "",
            "reachable": true,
            "status": local,
        }));
    }

    HttpResponse::Ok().json(serde_json::json!({
        "backend": "raft",
        "this_node": this_node,
        "nodes": nodes,
    }))
}
