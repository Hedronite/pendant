use std::convert::Infallible;
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::stream::{self, Stream};
use pendant_herdr::{CreateWorktree, HerdrError};
use serde::Deserialize;
use serde_json::json;

use crate::static_files;
use crate::AppState;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/v1/health", get(health))
        .route("/v1/snapshot", get(snapshot))
        .route("/v1/worktrees", get(list_worktrees).post(create_worktree))
        .route("/v1/agents/{id}", get(agent_detail))
        .route("/v1/agents/{id}/prompt", post(agent_prompt))
        .route("/v1/agents/{id}/keys", post(agent_keys))
        .route("/v1/events", get(events_inbox))
        .route("/v1/events/stream", get(events_sse))
        .fallback(static_files::static_handler)
        .with_state(state)
}

#[derive(Deserialize)]
struct WorktreeQuery {
    workspace_id: Option<String>,
}

#[derive(Deserialize)]
struct PromptBody {
    #[serde(alias = "text")]
    prompt: String,
}

#[derive(Deserialize)]
struct KeysBody {
    keys: Vec<String>,
}

async fn health(State(state): State<AppState>) -> impl IntoResponse {
    let herdr_up = state.herdr.health().await;
    let status = if herdr_up {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (
        status,
        Json(json!({
            "ok": herdr_up,
            "herdr": if herdr_up { "up" } else { "down" },
            "socket": state.herdr.socket_path().display().to_string(),
        })),
    )
}

async fn snapshot(State(state): State<AppState>) -> Response {
    match state.herdr.snapshot().await {
        Ok(snap) => Json(json!({
            "agents": snap.agents,
            "workspaces": snap.workspaces,
            "raw": snap.raw,
        }))
        .into_response(),
        Err(err) => herdr_error(err),
    }
}

async fn list_worktrees(
    State(state): State<AppState>,
    Query(query): Query<WorktreeQuery>,
) -> Response {
    match state
        .herdr
        .worktree_list(query.workspace_id.as_deref())
        .await
    {
        Ok(value) => Json(value).into_response(),
        Err(err) => herdr_error(err),
    }
}

async fn create_worktree(
    State(state): State<AppState>,
    Json(body): Json<CreateWorktree>,
) -> Response {
    match state.herdr.worktree_create(&body).await {
        Ok(value) => Json(value).into_response(),
        Err(err) => herdr_error(err),
    }
}

async fn agent_detail(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let snapshot = match state.herdr.snapshot().await {
        Ok(snap) => snap,
        Err(err) => return herdr_error(err),
    };
    let agent = snapshot.agent_by_id(&id).cloned();
    let output = match state.herdr.agent_read(&id).await {
        Ok(value) => Some(value),
        Err(HerdrError::Rpc { .. }) => None,
        Err(err) if err.is_transport() => return herdr_error(err),
        Err(_) => None,
    };
    let get = match state.herdr.agent_get(&id).await {
        Ok(value) => Some(value),
        Err(_) => None,
    };
    Json(json!({
        "id": id,
        "agent": agent,
        "get": get,
        "output": output,
        "blocked": agent.as_ref().map(|a| a.status == "blocked").unwrap_or(false),
    }))
    .into_response()
}

async fn agent_prompt(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<PromptBody>,
) -> Response {
    if body.prompt.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error":{"code":"empty_prompt","message":"prompt must not be empty"}})),
        )
            .into_response();
    }
    match state.herdr.agent_prompt(&id, &body.prompt).await {
        Ok(value) => Json(value).into_response(),
        Err(err) => herdr_error(err),
    }
}

async fn agent_keys(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<KeysBody>,
) -> Response {
    if body.keys.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error":{"code":"empty_keys","message":"keys must not be empty"}})),
        )
            .into_response();
    }
    match state.herdr.agent_send_keys(&id, &body.keys).await {
        Ok(value) => Json(value).into_response(),
        Err(err) => herdr_error(err),
    }
}

async fn events_inbox(State(state): State<AppState>) -> Response {
    match state.herdr.inbox().await {
        Ok(items) => Json(json!({ "items": items })).into_response(),
        Err(err) => herdr_error(err),
    }
}

async fn events_sse(
    State(state): State<AppState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let herdr = state.herdr;
    let stream = stream::unfold((herdr, None::<String>), |(herdr, last)| async move {
        if last.is_some() {
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
        let event = match herdr.inbox().await {
            Ok(items) => {
                let payload = json!({ "type": "inbox", "items": items }).to_string();
                if last.as_ref() == Some(&payload) {
                    return Some((Ok(Event::default().comment("unchanged")), (herdr, last)));
                }
                (
                    Ok(Event::default().event("inbox").data(payload.clone())),
                    (herdr, Some(payload)),
                )
            }
            Err(err) => (
                Ok(Event::default().event("error").data(err.to_string())),
                (herdr, last),
            ),
        };
        Some(event)
    });
    Sse::new(stream).keep_alive(KeepAlive::default())
}

fn herdr_error(err: HerdrError) -> Response {
    let (status, code) = match &err {
        HerdrError::Connect { .. }
        | HerdrError::CliMissing
        | HerdrError::Timeout
        | HerdrError::EmptyResponse
        | HerdrError::Unavailable { .. } => (StatusCode::BAD_GATEWAY, "herdr_unavailable"),
        HerdrError::Rpc { code, .. } if code == "agent_blocked" => {
            (StatusCode::CONFLICT, "agent_blocked")
        }
        HerdrError::Rpc { .. } => (StatusCode::BAD_GATEWAY, "herdr_rpc"),
        HerdrError::CliFailed { .. } => (StatusCode::BAD_GATEWAY, "herdr_cli"),
        HerdrError::UnsupportedMethod(_) => (StatusCode::NOT_IMPLEMENTED, "unsupported"),
        _ => (StatusCode::INTERNAL_SERVER_ERROR, "herdr_error"),
    };
    (
        status,
        Json(json!({
            "error": { "code": code, "message": err.to_string() }
        })),
    )
        .into_response()
}
