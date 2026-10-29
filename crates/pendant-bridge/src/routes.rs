use std::convert::Infallible;
use std::time::Duration;

use axum::extract::ws::WebSocketUpgrade;
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

use crate::session::{approve_key, scratch_socket};
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
        .route("/v1/ws", get(ws_upgrade))
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

async fn ws_upgrade(ws: WebSocketUpgrade, State(state): State<AppState>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| crate::session::run_socket(socket, state))
}

fn refuse_write() -> Response {
    (
        StatusCode::FORBIDDEN,
        Json(json!({
            "error": {
                "code": "scratch_only",
                "message": "writes require a named scratch session"
            }
        })),
    )
        .into_response()
}

async fn create_worktree(
    State(state): State<AppState>,
    Json(body): Json<CreateWorktree>,
) -> Response {
    if !scratch_socket(state.herdr.socket_path()) {
        return refuse_write();
    }
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
    if !scratch_socket(state.herdr.socket_path()) {
        return refuse_write();
    }
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
    if !scratch_socket(state.herdr.socket_path()) {
        return refuse_write();
    }
    if body.keys.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error":{"code":"empty_keys","message":"keys must not be empty"}})),
        )
            .into_response();
    }
    if body.keys.iter().any(|key| !approve_key(key)) {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({
                "error": {
                    "code": "key_refused",
                    "message": "key is outside the approve bar"
                }
            })),
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

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use pendant_herdr::HerdrClient;
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
    use tokio::net::{TcpListener, TcpStream, UnixListener};

    use crate::AppState;

    async fn http(port: u16, method: &str, path: &str, body: &str) -> (u16, String) {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        let req = format!(
            "{method} {path} HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(req.as_bytes()).await.unwrap();
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).await.unwrap();
        let text = String::from_utf8_lossy(&buf).to_string();
        let status = text.split_whitespace().nth(1).unwrap().parse().unwrap();
        (status, text)
    }

    fn spawn_mock(path: &std::path::Path, seen: Arc<Mutex<Vec<String>>>) {
        let path = path.to_path_buf();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                let seen = seen.clone();
                tokio::spawn(async move {
                    let mut reader = BufReader::new(stream);
                    let mut line = String::new();
                    if reader.read_line(&mut line).await.unwrap_or(0) == 0 {
                        return;
                    }
                    let req: serde_json::Value =
                        serde_json::from_str(line.trim()).unwrap_or(serde_json::json!({}));
                    if let Some(method) = req.get("method").and_then(|m| m.as_str()) {
                        seen.lock().unwrap().push(method.to_string());
                    }
                    let id = req.get("id").and_then(|v| v.as_str()).unwrap_or("");
                    let result = if req["method"] == "session.snapshot" {
                        serde_json::json!({
                            "type": "session_snapshot",
                            "agents": [{
                                "pane_id": "w1:p1",
                                "agent_status": "idle",
                                "workspace_id": "w1"
                            }],
                            "workspaces": []
                        })
                    } else {
                        serde_json::json!({"ok": true})
                    };
                    let resp = serde_json::json!({"id": id, "result": result});
                    let mut stream = reader.into_inner();
                    let _ = stream.write_all(format!("{resp}\n").as_bytes()).await;
                });
            }
        });
    }

    async fn serve_on(path: &std::path::Path) -> u16 {
        let state = AppState {
            herdr: HerdrClient::new(path),
            queue_cap: 8,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let _ = axum::serve(listener, crate::app(state)).await;
        });
        port
    }

    #[tokio::test]
    async fn gets_on_the_default_socket_still_return_data_and_posts_are_refused() {
        let live = std::path::PathBuf::from("/tmp/pbg1-live/.config/herdr/herdr.sock");
        let seen = Arc::new(Mutex::new(Vec::new()));
        spawn_mock(&live, seen.clone());
        let port = serve_on(&live).await;
        let (status, body) = http(port, "GET", "/v1/snapshot", "").await;
        assert_eq!(status, 200);
        assert!(body.contains("w1:p1"));
        let (status, body) = http(
            port,
            "POST",
            "/v1/agents/w1:p1/prompt",
            r#"{"prompt":"hello"}"#,
        )
        .await;
        assert_eq!(status, 403);
        assert!(body.contains("scratch_only"));
        assert!(!seen.lock().unwrap().iter().any(|m| m == "agent.prompt"));
        let _ = body;
    }

    #[tokio::test]
    async fn scratch_prompt_succeeds_and_a_key_outside_the_bar_is_refused() {
        let scratch =
            std::path::PathBuf::from("/tmp/pbg1-scratch/.config/herdr/sessions/g1/herdr.sock");
        let seen = Arc::new(Mutex::new(Vec::new()));
        spawn_mock(&scratch, seen.clone());
        let port = serve_on(&scratch).await;
        let (status, _) = http(
            port,
            "POST",
            "/v1/agents/w1:p1/prompt",
            r#"{"prompt":"hello"}"#,
        )
        .await;
        assert_eq!(status, 200);
        assert!(seen.lock().unwrap().iter().any(|m| m == "agent.prompt"));
        let (status, _) = http(port, "POST", "/v1/worktrees", "{}").await;
        assert_eq!(status, 200);
        let (status, body) =
            http(port, "POST", "/v1/agents/w1:p1/keys", r#"{"keys":["rm"]}"#).await;
        assert_eq!(status, 403);
        assert!(body.contains("key_refused"));
        assert!(!seen.lock().unwrap().iter().any(|m| m == "agent.send_keys"));
    }
}
