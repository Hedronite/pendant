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
use serde_json::{json, Value};

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

/// herdr targets look like `wN:p2`, agent names, or session ids — never
/// flags or paths. Same rule as the Python prototype bridge.
fn is_valid_target(target: &str) -> bool {
    !target.is_empty()
        && target.len() <= 96
        && !target.starts_with('-')
        && target
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | ':' | '@' | '/' | '-'))
}

fn bad_request(code: &str, message: impl Into<String>) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({"error":{"code":code,"message":message.into()}})),
    )
        .into_response()
}

/// Approve/deny surface only. send-keys is key injection by design; keep the
/// spike whitelist tight so the bridge can't become an arbitrary keyboard.
/// Digits 1-4 cover numbered option prompts (PWA action bar sends them).
const KEY_WHITELIST: &[&str] = &[
    "y", "n", "1", "2", "3", "4", "enter", "esc", "escape", "tab", "space", "up", "down", "left",
    "right", "ctrl+c", "ctrl+d", "ctrl+z",
];

/// PWA sends tmux-style `ctrl-c`; herdr wants `ctrl+c`. Lowercase too.
fn normalize_key(key: &str) -> String {
    let k = key.trim().to_lowercase();
    match k.strip_prefix("ctrl-") {
        Some(rest) => format!("ctrl+{rest}"),
        None => k,
    }
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
    if !is_valid_target(&id) {
        return bad_request("invalid_target", "invalid target");
    }
    let snapshot = match state.herdr.snapshot().await {
        Ok(snap) => snap,
        Err(err) => return herdr_error(err),
    };
    let agent = snapshot.agent_by_id(&id).cloned();
    // Socket returns {type:"pane_read", read:{text}}, CLI fallback returns
    // {type:"agent_read", text}; the PWA wants one stable string.
    let output = match state.herdr.agent_read(&id).await {
        Ok(value) => value
            .pointer("/read/text")
            .or_else(|| value.get("text"))
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
            .or_else(|| Some(value.to_string())),
        Err(HerdrError::Rpc { .. }) => None,
        Err(err) if err.is_transport() => return herdr_error(err),
        Err(_) => None,
    };
    let get = state.herdr.agent_get(&id).await.ok();
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
    if !is_valid_target(&id) {
        return bad_request("invalid_target", "invalid target");
    }
    if body.prompt.trim().is_empty() {
        return bad_request("empty_prompt", "prompt must not be empty");
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
    if !is_valid_target(&id) {
        return bad_request("invalid_target", "invalid target");
    }
    if body.keys.is_empty() || body.keys.len() > 8 {
        return bad_request("empty_keys", "keys must be a list of 1..8 key names");
    }
    let keys: Vec<String> = body.keys.iter().map(|k| normalize_key(k)).collect();
    // "focus" is pane chrome, not key injection: focus the tab owning the
    // pane (herdr 0.9.x has tab focus, no focus-pane call). Must be alone.
    if keys.iter().any(|k| k == "focus") {
        if keys.len() != 1 {
            return bad_request(
                "focus_not_alone",
                "focus cannot be combined with other keys",
            );
        }
        return focus_pane_tab(&state, &id).await;
    }
    if let Some(bad) = keys.iter().find(|k| !KEY_WHITELIST.contains(&k.as_str())) {
        return bad_request(
            "key_not_whitelisted",
            format!("key not in approve whitelist: {bad}"),
        );
    }
    match state.herdr.agent_send_keys(&id, &keys).await {
        Ok(value) => Json(value).into_response(),
        Err(err) => herdr_error(err),
    }
}

/// Map a pane target to its owning tab via the snapshot, then `tab.focus`.
async fn focus_pane_tab(state: &AppState, target: &str) -> Response {
    let snap = match state.herdr.snapshot().await {
        Ok(snap) => snap,
        Err(err) => return herdr_error(err),
    };
    let tab = snap
        .agents
        .iter()
        .find(|a| a.pane_id == target || a.id == target)
        .and_then(|a| a.tab_id.clone());
    match tab {
        Some(tab) => match state.herdr.tab_focus(&tab).await {
            Ok(_) => Json(json!({ "ok": true, "target": target, "tab": tab })).into_response(),
            Err(err) => herdr_error(err),
        },
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({"error":{"code":"pane_not_found","message":format!("pane not found in snapshot: {target}")}})),
        )
            .into_response(),
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
        | HerdrError::EmptyResponse => (StatusCode::BAD_GATEWAY, "herdr_unavailable"),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_validation_matches_prototype_rule() {
        assert!(is_valid_target("w1:p2"));
        assert!(is_valid_target("agent-name_1.2"));
        assert!(is_valid_target("oma@castle"));
        assert!(!is_valid_target(""));
        assert!(!is_valid_target("--workspace"));
        assert!(!is_valid_target("bad target"));
        assert!(!is_valid_target("a;b"));
        assert!(!is_valid_target(&"x".repeat(97)));
    }

    #[test]
    fn normalize_key_tmux_style_to_herdr() {
        assert_eq!(normalize_key("ctrl-c"), "ctrl+c");
        assert_eq!(normalize_key(" CTRL-D "), "ctrl+d");
        assert_eq!(normalize_key("Y"), "y");
        assert_eq!(normalize_key("enter"), "enter");
    }

    #[test]
    fn whitelist_covers_approve_bar_only() {
        for k in ["y", "n", "1", "2", "3", "4", "enter", "esc"] {
            assert!(KEY_WHITELIST.contains(&k), "{k} should be allowed");
        }
        for k in ["rm", "ctrl+shift+c", "f5", "a", "ctrl+o"] {
            assert!(!KEY_WHITELIST.contains(&k), "{k} must be rejected");
        }
    }
}
