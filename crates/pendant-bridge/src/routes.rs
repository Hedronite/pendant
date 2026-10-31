use std::convert::Infallible;
use std::time::Duration;

use axum::extract::ws::WebSocketUpgrade;
use axum::extract::{Path, Query, Request, State};
use axum::http::header::{HOST, ORIGIN};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::{from_fn_with_state, Next};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::stream::{self, Stream};
use pendant_herdr::{CreateWorktree, HerdrError};
use serde::Deserialize;
use serde_json::json;

use crate::pairing::{self, unpaired_log, AuthError};
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
        .route("/v1/agents/{id}/approve", post(agent_approve))
        .route("/v1/pair/qr", get(pair_qr))
        .route("/v1/challenge", get(issue_challenge))
        .route("/v1/pair", post(pair_device))
        .route("/v1/grant", post(open_grant))
        .route("/v1/devices/{id}/revoke", post(revoke_device))
        .route("/v1/events", get(events_inbox))
        .route("/v1/events/stream", get(events_sse))
        .route("/v1/ws", get(ws_upgrade))
        .fallback(static_files::static_handler)
        .layer(from_fn_with_state(state.clone(), require_host))
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

async fn require_host(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let host = req
        .headers()
        .get(HOST)
        .and_then(|value| value.to_str().ok());
    if !host_allowed(host, state.bind, state.serve_origin.as_deref()) {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({
                "error": { "code": "host_refused", "message": "host is not allowed" }
            })),
        )
            .into_response();
    }
    next.run(req).await
}

async fn ws_upgrade(
    State(state): State<AppState>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    let origin = headers.get(ORIGIN).and_then(|value| value.to_str().ok());
    if !origin_allowed(origin, state.bind, state.serve_origin.as_deref()) {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({
                "error": { "code": "origin_refused", "message": "origin is not allowed" }
            })),
        )
            .into_response();
    }
    ws.on_upgrade(move |socket| crate::session::run_socket(socket, state))
        .into_response()
}

/// `http://127.0.0.1:{port}`, `http://localhost:{port}`, or the configured Serve origin.
pub fn origin_allowed(
    origin: Option<&str>,
    bind: std::net::SocketAddr,
    serve_origin: Option<&str>,
) -> bool {
    let Some(origin) = origin.and_then(parse_origin) else {
        return false;
    };
    if loopback_http(&origin, bind) {
        return true;
    }
    serve_origin
        .and_then(parse_origin)
        .is_some_and(|want| want == origin)
}

pub fn host_allowed(
    host: Option<&str>,
    bind: std::net::SocketAddr,
    serve_origin: Option<&str>,
) -> bool {
    let Some(host) = host.map(str::trim).filter(|h| !h.is_empty()) else {
        return false;
    };
    let port = bind.port();
    if host == format!("127.0.0.1:{port}")
        || host == format!("localhost:{port}")
        || host == format!("[::1]:{port}")
    {
        return true;
    }
    let Some(origin) = serve_origin.and_then(parse_origin) else {
        return false;
    };
    let with_port = format!("{}:{}", origin.host, origin.port);
    if host.eq_ignore_ascii_case(&with_port) {
        return true;
    }
    host.eq_ignore_ascii_case(&origin.host) && (origin.port == 80 || origin.port == 443)
}

#[derive(PartialEq)]
struct OriginParts {
    scheme: String,
    host: String,
    port: u16,
}

fn parse_origin(raw: &str) -> Option<OriginParts> {
    let raw = raw.trim().trim_end_matches('/');
    let (scheme, rest) = raw.split_once("://")?;
    if rest.is_empty() || rest.contains('/') {
        return None;
    }
    let scheme = scheme.to_ascii_lowercase();
    let (host, port) = if let Some(rest) = rest.strip_prefix('[') {
        let (host, after) = rest.split_once(']')?;
        if host.is_empty() {
            return None;
        }
        let port = if after.is_empty() {
            if scheme == "https" {
                443
            } else {
                80
            }
        } else {
            after.strip_prefix(':')?.parse().ok()?
        };
        (format!("[{}]", host.to_ascii_lowercase()), port)
    } else if let Some((host, port)) = rest.rsplit_once(':') {
        if host.is_empty() || port.is_empty() || host.contains(':') {
            return None;
        }
        (host.to_ascii_lowercase(), port.parse().ok()?)
    } else {
        let port = if scheme == "https" { 443 } else { 80 };
        (rest.to_ascii_lowercase(), port)
    };
    Some(OriginParts { scheme, host, port })
}

fn loopback_http(origin: &OriginParts, bind: std::net::SocketAddr) -> bool {
    origin.scheme == "http"
        && origin.port == bind.port()
        && matches!(origin.host.as_str(), "127.0.0.1" | "localhost" | "[::1]")
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
    headers: HeaderMap,
    Json(body): Json<CreateWorktree>,
) -> Response {
    if !scratch_socket(state.herdr.socket_path()) {
        return refuse_write();
    }
    let canonical = serde_json::to_string(&body).unwrap_or_default();
    let message = signed_message(&headers, "POST", "/v1/worktrees", &canonical);
    if let Err(response) = gate_write(&state, &headers, &message) {
        return response;
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
    headers: HeaderMap,
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
    let path = format!("/v1/agents/{id}/prompt");
    let message = signed_message(&headers, "POST", &path, body.prompt.trim());
    if let Err(response) = gate_write(&state, &headers, &message) {
        return response;
    }
    match state.herdr.agent_prompt(&id, &body.prompt).await {
        Ok(value) => Json(value).into_response(),
        Err(err) => herdr_error(err),
    }
}

async fn agent_keys(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
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
    let path = format!("/v1/agents/{id}/keys");
    let canonical = body.keys.join("\n");
    let message = signed_message(&headers, "POST", &path, &canonical);
    if let Err(response) = gate_write(&state, &headers, &message) {
        return response;
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

#[derive(Deserialize)]
struct ApproveBody {
    decision: String,
}

async fn agent_approve(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<ApproveBody>,
) -> Response {
    if !scratch_socket(state.herdr.socket_path()) {
        return refuse_write();
    }
    let path = format!("/v1/agents/{id}/approve");
    let message = signed_message(&headers, "POST", &path, body.decision.trim());
    if let Err(response) = gate_write(&state, &headers, &message) {
        return response;
    }
    let decision = body.decision.trim().to_string();
    if !approve_key(&decision) {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({
                "error": { "code": "key_refused", "message": "key is outside the approve bar" }
            })),
        )
            .into_response();
    }
    match state.herdr.agent_send_keys(&id, &[decision]).await {
        Ok(value) => Json(value).into_response(),
        Err(err) => herdr_error(err),
    }
}

async fn pair_qr(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(response) = local_only(&state, &headers) {
        return response;
    }
    let nonce = state.pairing.issue_nonce();
    Json(json!({
        "v": 1,
        "nonce": nonce,
        "payload": pairing::Pairing::qr_payload(&nonce),
        "algorithm": pairing::DEVICE_SIGNATURE,
    }))
    .into_response()
}

async fn issue_challenge(State(state): State<AppState>) -> impl IntoResponse {
    Json(json!({
        "challenge": state.pairing.issue_challenge(),
    }))
}

#[derive(Deserialize)]
struct PairBody {
    device_id: String,
    public_key: String,
    nonce: String,
    sig: String,
    #[serde(default)]
    takeover: bool,
}

async fn pair_device(State(state): State<AppState>, Json(body): Json<PairBody>) -> Response {
    match state.pairing.pair(
        &body.device_id,
        &body.public_key,
        &body.nonce,
        &body.sig,
        body.takeover,
    ) {
        Ok(()) => Json(json!({"ok": true, "takeover": pairing::TAKEOVER})).into_response(),
        Err(err) => auth_error(err),
    }
}

#[derive(Deserialize)]
struct GrantBody {
    device_id: String,
    sig: String,
    challenge: String,
}

async fn open_grant(State(state): State<AppState>, Json(body): Json<GrantBody>) -> Response {
    match state
        .pairing
        .open_grant(&body.device_id, &body.sig, &body.challenge)
    {
        Ok(()) => Json(json!({"ok": true})).into_response(),
        Err(err) => auth_error(err),
    }
}

async fn revoke_device(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = local_only(&state, &headers) {
        return response;
    }
    let Some(signer) = header_value(&headers, "x-pendant-device") else {
        return auth_error(AuthError::BadSignature);
    };
    let Some(sig) = header_value(&headers, "x-pendant-signature") else {
        return auth_error(AuthError::BadSignature);
    };
    let Some(challenge) = header_value(&headers, "x-pendant-challenge") else {
        return auth_error(AuthError::BadSignature);
    };
    match state.pairing.revoke(&id, signer, sig, challenge) {
        Ok(()) => Json(json!({
            "ok": true,
            "revoked": id,
            "real_pairings": state.pairing.real_pairings(),
        }))
        .into_response(),
        Err(err) => auth_error(err),
    }
}

fn signed_message(headers: &HeaderMap, method: &str, path: &str, canonical: &str) -> Vec<u8> {
    let device = header_value(headers, "x-pendant-device").unwrap_or("");
    let challenge = header_value(headers, "x-pendant-challenge").unwrap_or("");
    pairing::sign_message(device, method, path, challenge, canonical)
}

fn gate_write(state: &AppState, headers: &HeaderMap, message: &[u8]) -> Result<(), Response> {
    let host = headers.get(HOST).and_then(|value| value.to_str().ok());
    let device = header_value(headers, "x-pendant-device");
    let sig = header_value(headers, "x-pendant-signature");
    let challenge = header_value(headers, "x-pendant-challenge").unwrap_or("");
    if via_serve(host, state.serve_origin.as_deref())
        && !device.is_some_and(|id| state.pairing.is_paired(id))
    {
        let line = unpaired_log(host.unwrap_or("serve"));
        tracing::warn!("{line}");
        return Err(forbid("unpaired", &line));
    }
    let (Some(device), Some(sig)) = (device, sig) else {
        return Err(forbid(
            "bad_signature",
            "device signature is missing or wrong",
        ));
    };
    match state.pairing.authorize(device, message, sig, challenge) {
        Ok(()) => Ok(()),
        Err(err) => Err(auth_error(err)),
    }
}

fn via_serve(host: Option<&str>, serve_origin: Option<&str>) -> bool {
    let Some(host) = host.map(str::trim).filter(|value| !value.is_empty()) else {
        return false;
    };
    if host.starts_with("127.0.0.1") || host.starts_with("localhost") || host.starts_with("[::1]") {
        return false;
    }
    let Some(origin) = serve_origin.and_then(parse_origin) else {
        return false;
    };
    let with_port = format!("{}:{}", origin.host, origin.port);
    host.eq_ignore_ascii_case(&with_port)
        || (host.eq_ignore_ascii_case(&origin.host) && (origin.port == 80 || origin.port == 443))
}

fn local_only(state: &AppState, headers: &HeaderMap) -> Result<(), Response> {
    let host = headers.get(HOST).and_then(|value| value.to_str().ok());
    if via_serve(host, state.serve_origin.as_deref()) {
        Err(forbid("local_only", "this route is only on the host"))
    } else {
        Ok(())
    }
}

fn header_value<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn forbid(code: &str, message: &str) -> Response {
    (
        StatusCode::FORBIDDEN,
        Json(json!({"error": {"code": code, "message": message}})),
    )
        .into_response()
}

fn auth_error(err: AuthError) -> Response {
    let (code, message) = match err {
        AuthError::BadSignature => ("bad_signature", "device signature is missing or wrong"),
        AuthError::GrantRequired => ("grant_required", "biometric grant is not open"),
        AuthError::GrantReleased => ("grant_released", "biometric grant released"),
        AuthError::Revoked => ("revoked", "device is revoked"),
        AuthError::TakeoverOff => ("takeover_off", "takeover is off"),
        AuthError::Unpaired => ("unpaired", "unpaired device"),
    };
    forbid(code, message)
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

    use crate::pairing;
    use crate::AppState;

    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    async fn http(port: u16, method: &str, path: &str, body: &str) -> (u16, String) {
        http_host(port, method, path, body, &format!("127.0.0.1:{port}")).await
    }

    async fn http_host(
        port: u16,
        method: &str,
        path: &str,
        body: &str,
        host: &str,
    ) -> (u16, String) {
        http_headers(port, method, path, body, host, &[]).await
    }

    async fn http_headers(
        port: u16,
        method: &str,
        path: &str,
        body: &str,
        host: &str,
        extra: &[(&str, &str)],
    ) -> (u16, String) {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        let mut headers = String::new();
        for (name, value) in extra {
            headers.push_str(name);
            headers.push_str(": ");
            headers.push_str(value);
            headers.push_str("\r\n");
        }
        let req = format!(
            "{method} {path} HTTP/1.1\r\nHost: {host}\r\n{headers}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(req.as_bytes()).await.unwrap();
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).await.unwrap();
        let text = String::from_utf8_lossy(&buf).to_string();
        let status = text.split_whitespace().nth(1).unwrap().parse().unwrap();
        (status, text)
    }

    fn response_json(raw: &str) -> serde_json::Value {
        let body = raw.split("\r\n\r\n").nth(1).unwrap_or("");
        serde_json::from_str(body).unwrap_or(serde_json::json!({}))
    }

    async fn pair_only(port: u16) -> pairing::TestDevice {
        let (status, raw) = http(port, "GET", "/v1/pair/qr", "").await;
        assert_eq!(status, 200);
        let qr = response_json(&raw);
        let nonce = qr["nonce"].as_str().unwrap();
        assert!(qr["payload"]
            .as_str()
            .unwrap()
            .starts_with("pendant-pair:v1:"));
        assert!(!raw.contains("private_key"));
        let phone = pairing::TestDevice::generate("phone-1");
        let public = phone.public_b64();
        let sig = phone.sign_b64(&pairing::pair_message(&phone.id, &public, nonce));
        let pair_body = serde_json::json!({
            "device_id": phone.id,
            "public_key": public,
            "nonce": nonce,
            "sig": sig,
        })
        .to_string();
        let (status, _) = http(port, "POST", "/v1/pair", &pair_body).await;
        assert_eq!(status, 200);
        phone
    }

    async fn fresh_challenge(port: u16) -> String {
        let (status, raw) = http(port, "GET", "/v1/challenge", "").await;
        assert_eq!(status, 200);
        response_json(&raw)["challenge"]
            .as_str()
            .unwrap()
            .to_string()
    }

    async fn enroll(port: u16) -> pairing::TestDevice {
        let phone = pair_only(port).await;
        let challenge = fresh_challenge(port).await;
        let grant_sig = phone.sign_b64(&pairing::grant_message(&phone.id, &challenge));
        let grant_body = serde_json::json!({
            "device_id": phone.id,
            "sig": grant_sig,
            "challenge": challenge,
        })
        .to_string();
        let (status, _) = http(port, "POST", "/v1/grant", &grant_body).await;
        assert_eq!(status, 200);
        phone
    }

    async fn signed(
        port: u16,
        phone: &pairing::TestDevice,
        method: &str,
        path: &str,
        canonical: &str,
        body: &str,
    ) -> (u16, String) {
        let challenge = fresh_challenge(port).await;
        let sig = phone.sign_b64(&pairing::sign_message(
            &phone.id, method, path, &challenge, canonical,
        ));
        http_headers(
            port,
            method,
            path,
            body,
            &format!("127.0.0.1:{port}"),
            &[
                ("x-pendant-device", phone.id.as_str()),
                ("x-pendant-signature", sig.as_str()),
                ("x-pendant-challenge", challenge.as_str()),
            ],
        )
        .await
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
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let state = AppState {
            herdr: HerdrClient::new(path),
            queue_cap: 8,
            bind: addr,
            serve_origin: None,
            pairing: pairing::Pairing::new(),
        };
        serve_state(listener, state).await
    }

    async fn serve_with(
        path: &std::path::Path,
        pairing: pairing::Pairing,
        serve_origin: Option<String>,
    ) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let state = AppState {
            herdr: HerdrClient::new(path),
            queue_cap: 8,
            bind: addr,
            serve_origin,
            pairing,
        };
        serve_state(listener, state).await
    }

    async fn serve_state(listener: TcpListener, state: AppState) -> u16 {
        let port = state.bind.port();
        tokio::spawn(async move {
            let _ = axum::serve(listener, crate::app(state)).await;
        });
        port
    }

    #[tokio::test]
    async fn gets_on_the_default_socket_still_return_data_and_posts_are_refused() {
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        let live = dir.path().join(".config/herdr/herdr.sock");
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
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        let scratch = dir.path().join(".config/herdr/sessions/g1/herdr.sock");
        let seen = Arc::new(Mutex::new(Vec::new()));
        spawn_mock(&scratch, seen.clone());
        let port = serve_on(&scratch).await;
        let phone = enroll(port).await;
        let (status, _) = signed(
            port,
            &phone,
            "POST",
            "/v1/agents/w1:p1/prompt",
            "hello",
            r#"{"prompt":"hello"}"#,
        )
        .await;
        assert_eq!(status, 200);
        assert!(seen.lock().unwrap().iter().any(|m| m == "agent.prompt"));
        let tree: pendant_herdr::CreateWorktree = serde_json::from_str("{}").unwrap();
        let tree_body = serde_json::to_string(&tree).unwrap();
        let (status, _) = signed(
            port,
            &phone,
            "POST",
            "/v1/worktrees",
            &tree_body,
            &tree_body,
        )
        .await;
        assert_eq!(status, 200);
        let (status, body) = signed(
            port,
            &phone,
            "POST",
            "/v1/agents/w1:p1/keys",
            "rm",
            r#"{"keys":["rm"]}"#,
        )
        .await;
        assert_eq!(status, 403);
        assert!(body.contains("key_refused"));
        assert!(!seen.lock().unwrap().iter().any(|m| m == "agent.send_keys"));
    }

    #[tokio::test]
    async fn foreign_host_is_refused() {
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        let live = dir.path().join(".config/herdr/herdr.sock");
        spawn_mock(&live, Arc::new(Mutex::new(Vec::new())));
        let port = serve_on(&live).await;
        let (status, body) = http_host(port, "GET", "/v1/snapshot", "", "foreign.invalid").await;
        assert_eq!(status, 403);
        assert!(body.contains("host_refused"));
        let (status, _) = http(port, "GET", "/v1/snapshot", "").await;
        assert_eq!(status, 200);
    }

    #[tokio::test]
    async fn websocket_accepts_loopback_origin_and_rejects_a_foreign_origin() {
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;

        let dir = tempfile::tempdir_in("/tmp").unwrap();
        let sock = dir.path().join(".config/herdr/herdr.sock");
        spawn_mock(&sock, Arc::new(Mutex::new(Vec::new())));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let port = addr.port();
        let state = AppState {
            herdr: HerdrClient::new(&sock),
            queue_cap: 4,
            bind: addr,
            serve_origin: Some("https://serve.invalid".into()),
            pairing: pairing::Pairing::new(),
        };
        tokio::spawn(async move {
            let _ = axum::serve(listener, crate::app(state)).await;
        });

        let url = format!("ws://127.0.0.1:{port}/v1/ws");
        let mut ok = url.clone().into_client_request().unwrap();
        ok.headers_mut().insert(
            "Origin",
            format!("http://127.0.0.1:{port}").parse().unwrap(),
        );
        tokio_tungstenite::connect_async(ok).await.unwrap();

        let mut foreign = url.into_client_request().unwrap();
        foreign
            .headers_mut()
            .insert("Origin", "http://foreign.invalid".parse().unwrap());
        assert!(tokio_tungstenite::connect_async(foreign).await.is_err());
    }

    #[tokio::test]
    async fn websocket_accepts_configured_serve_origin() {
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::set_var("PENDANT_SERVE_ORIGIN", "https://serve.invalid");
        let origin = std::env::var("PENDANT_SERVE_ORIGIN").unwrap();
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        let sock = dir.path().join(".config/herdr/herdr.sock");
        spawn_mock(&sock, Arc::new(Mutex::new(Vec::new())));
        let port = serve_with(&sock, pairing::Pairing::new(), Some(origin.clone())).await;
        let mut req = format!("ws://127.0.0.1:{port}/v1/ws")
            .into_client_request()
            .unwrap();
        req.headers_mut().insert("Origin", origin.parse().unwrap());
        tokio_tungstenite::connect_async(req).await.unwrap();
        std::env::remove_var("PENDANT_SERVE_ORIGIN");
    }

    #[tokio::test]
    async fn websocket_refuses_a_missing_origin() {
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        let sock = dir.path().join(".config/herdr/herdr.sock");
        spawn_mock(&sock, Arc::new(Mutex::new(Vec::new())));
        let port = serve_on(&sock).await;
        let req = format!("ws://127.0.0.1:{port}/v1/ws")
            .into_client_request()
            .unwrap();
        let err = tokio_tungstenite::connect_async(req).await.unwrap_err();
        match err {
            tokio_tungstenite::tungstenite::Error::Http(response) => {
                assert_eq!(response.status(), 403);
            }
            other => panic!("expected an http refusal, got {other}"),
        }
    }

    #[tokio::test]
    async fn unpaired_device_is_refused_through_serve() {
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        let scratch = dir.path().join(".config/herdr/sessions/g1/herdr.sock");
        spawn_mock(&scratch, Arc::new(Mutex::new(Vec::new())));
        let port = serve_with(
            &scratch,
            pairing::Pairing::new(),
            Some("https://serve.invalid".into()),
        )
        .await;
        let (status, body) = http_headers(
            port,
            "POST",
            "/v1/agents/w1:p1/prompt",
            r#"{"prompt":"hello"}"#,
            "serve.invalid",
            &[("x-pendant-device", "phone-x")],
        )
        .await;
        assert_eq!(status, 403);
        assert!(body.contains("unpaired device refused via=serve host=serve.invalid"));
        let line = body
            .lines()
            .find(|line| line.contains("unpaired device refused"))
            .unwrap();
        assert!(!line.contains("127.0.0.1"));
    }

    #[tokio::test]
    async fn write_routes_refuse_a_missing_or_wrong_signature() {
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        let scratch = dir.path().join(".config/herdr/sessions/g1/herdr.sock");
        spawn_mock(&scratch, Arc::new(Mutex::new(Vec::new())));
        let port = serve_on(&scratch).await;
        let tree: pendant_herdr::CreateWorktree = serde_json::from_str("{}").unwrap();
        let tree_body = serde_json::to_string(&tree).unwrap();
        let calls = [
            ("/v1/agents/w1:p1/prompt", "hello", r#"{"prompt":"hello"}"#),
            ("/v1/agents/w1:p1/keys", "y", r#"{"keys":["y"]}"#),
            ("/v1/agents/w1:p1/approve", "y", r#"{"decision":"y"}"#),
            ("/v1/worktrees", tree_body.as_str(), tree_body.as_str()),
        ];
        for (path, canonical, body) in calls {
            let (status, raw) = http(port, "POST", path, body).await;
            assert_eq!(status, 403, "{path}");
            assert!(raw.contains("bad_signature"), "{path}");
            let phone = pairing::TestDevice::generate("phone-1");
            let sig = phone.sign_b64(b"wrong");
            let (status, raw) = http_headers(
                port,
                "POST",
                path,
                body,
                &format!("127.0.0.1:{port}"),
                &[
                    ("x-pendant-device", "phone-1"),
                    ("x-pendant-signature", sig.as_str()),
                ],
            )
            .await;
            assert_eq!(status, 403, "{path} {canonical}");
            assert!(raw.contains("bad_signature"), "{path}");
        }
    }

    #[tokio::test]
    async fn write_routes_refuse_a_signature_without_a_grant() {
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        let scratch = dir.path().join(".config/herdr/sessions/g1/herdr.sock");
        spawn_mock(&scratch, Arc::new(Mutex::new(Vec::new())));
        let port = serve_on(&scratch).await;
        let phone = pair_only(port).await;
        let tree: pendant_herdr::CreateWorktree = serde_json::from_str("{}").unwrap();
        let tree_body = serde_json::to_string(&tree).unwrap();
        let calls = [
            ("/v1/agents/w1:p1/prompt", "hello", r#"{"prompt":"hello"}"#),
            ("/v1/agents/w1:p1/keys", "y", r#"{"keys":["y"]}"#),
            ("/v1/agents/w1:p1/approve", "y", r#"{"decision":"y"}"#),
            ("/v1/worktrees", tree_body.as_str(), tree_body.as_str()),
        ];
        for (path, canonical, body) in calls {
            let (status, raw) = signed(port, &phone, "POST", path, canonical, body).await;
            assert_eq!(status, 403, "{path}");
            assert!(raw.contains("grant_required"), "{path}");
        }
    }

    #[tokio::test]
    async fn grant_releases_on_timeout() {
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        let scratch = dir.path().join(".config/herdr/sessions/g1/herdr.sock");
        spawn_mock(&scratch, Arc::new(Mutex::new(Vec::new())));
        let port = serve_with(
            &scratch,
            pairing::Pairing::with_ttl(std::time::Duration::from_millis(30)),
            None,
        )
        .await;
        let phone = enroll(port).await;
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let (status, raw) = signed(
            port,
            &phone,
            "POST",
            "/v1/agents/w1:p1/prompt",
            "hello",
            r#"{"prompt":"hello"}"#,
        )
        .await;
        assert_eq!(status, 403);
        assert!(raw.contains("grant_released"));
    }

    #[tokio::test]
    async fn grant_releases_when_the_connection_drops() {
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        let scratch = dir.path().join(".config/herdr/sessions/g1/herdr.sock");
        spawn_mock(&scratch, Arc::new(Mutex::new(Vec::new())));
        let port = serve_on(&scratch).await;
        let phone = enroll(port).await;
        let mut req = format!("ws://127.0.0.1:{port}/v1/ws")
            .into_client_request()
            .unwrap();
        req.headers_mut().insert(
            "Origin",
            format!("http://127.0.0.1:{port}").parse().unwrap(),
        );
        let (mut ws, _) = tokio_tungstenite::connect_async(req).await.unwrap();
        let challenge = fresh_challenge(port).await;
        let sig = phone.sign_b64(&pairing::attach_message(&phone.id, &challenge));
        let attach = serde_json::json!({
            "v": 1,
            "pane": "w1:p1",
            "type": "attach",
            "device_id": phone.id,
            "sig": sig,
            "challenge": challenge,
        })
        .to_string();
        use futures_util::SinkExt;
        ws.send(tokio_tungstenite::tungstenite::Message::Text(attach.into()))
            .await
            .unwrap();
        drop(ws);
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let (status, raw) = signed(
            port,
            &phone,
            "POST",
            "/v1/agents/w1:p1/prompt",
            "hello",
            r#"{"prompt":"hello"}"#,
        )
        .await;
        assert_eq!(status, 403);
        assert!(raw.contains("grant_released"));
    }

    #[tokio::test]
    async fn takeover_stays_off() {
        assert!(!pairing::TAKEOVER);
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        let scratch = dir.path().join(".config/herdr/sessions/g1/herdr.sock");
        spawn_mock(&scratch, Arc::new(Mutex::new(Vec::new())));
        let port = serve_on(&scratch).await;
        let _first = pair_only(port).await;
        let (status, raw) = http(port, "GET", "/v1/pair/qr", "").await;
        assert_eq!(status, 200);
        let nonce = response_json(&raw)["nonce"].as_str().unwrap().to_string();
        let other = pairing::TestDevice::generate("phone-2");
        let public = other.public_b64();
        let sig = other.sign_b64(&pairing::pair_message(&other.id, &public, &nonce));
        let body = serde_json::json!({
            "device_id": other.id,
            "public_key": public,
            "nonce": nonce,
            "sig": sig,
            "takeover": true,
        })
        .to_string();
        let (status, raw) = http(port, "POST", "/v1/pair", &body).await;
        assert_eq!(status, 403);
        assert!(raw.contains("takeover_off"));
    }

    #[tokio::test]
    async fn lost_phone_revoke_is_refused_before_any_real_pairing() {
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        let scratch = dir.path().join(".config/herdr/sessions/g1/herdr.sock");
        spawn_mock(&scratch, Arc::new(Mutex::new(Vec::new())));
        let port = serve_on(&scratch).await;
        let phone = pair_only(port).await;
        let challenge = fresh_challenge(port).await;
        let sig = phone.sign_b64(&pairing::revoke_message(&phone.id, &challenge));
        let (status, raw) = http_headers(
            port,
            "POST",
            &format!("/v1/devices/{}/revoke", phone.id),
            "",
            &format!("127.0.0.1:{port}"),
            &[
                ("x-pendant-device", phone.id.as_str()),
                ("x-pendant-signature", sig.as_str()),
                ("x-pendant-challenge", challenge.as_str()),
            ],
        )
        .await;
        assert_eq!(status, 200);
        let body = response_json(&raw);
        assert_eq!(body["real_pairings"], 0);
        let grant_challenge = fresh_challenge(port).await;
        let grant_sig = phone.sign_b64(&pairing::grant_message(&phone.id, &grant_challenge));
        let grant_body = serde_json::json!({
            "device_id": phone.id,
            "sig": grant_sig,
            "challenge": grant_challenge,
        })
        .to_string();
        let (status, raw) = http(port, "POST", "/v1/grant", &grant_body).await;
        assert_eq!(status, 403);
        assert!(raw.contains("revoked"));
    }

    #[tokio::test]
    async fn qr_and_revoke_are_local_only_and_a_revoked_id_stays_tombstoned() {
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        let scratch = dir.path().join(".config/herdr/sessions/g1/herdr.sock");
        spawn_mock(&scratch, Arc::new(Mutex::new(Vec::new())));
        let port = serve_with(
            &scratch,
            pairing::Pairing::new(),
            Some("https://serve.invalid".into()),
        )
        .await;
        let (status, raw) = http_host(port, "GET", "/v1/pair/qr", "", "serve.invalid").await;
        assert_eq!(status, 403);
        assert!(raw.contains("local_only"));
        let phone = pair_only(port).await;
        let challenge = fresh_challenge(port).await;
        let sig = phone.sign_b64(&pairing::revoke_message(&phone.id, &challenge));
        let (status, raw) = http_headers(
            port,
            "POST",
            &format!("/v1/devices/{}/revoke", phone.id),
            "",
            "serve.invalid",
            &[
                ("x-pendant-device", phone.id.as_str()),
                ("x-pendant-signature", sig.as_str()),
                ("x-pendant-challenge", challenge.as_str()),
            ],
        )
        .await;
        assert_eq!(status, 403);
        assert!(raw.contains("local_only"));
        let (status, raw) = http(port, "GET", "/v1/pair/qr", "").await;
        assert_eq!(status, 200);
        let nonce = response_json(&raw)["nonce"].as_str().unwrap().to_string();
        let only_nonce = phone.sign_b64(format!("pendant-pair:v1:{nonce}").as_bytes());
        let public = phone.public_b64();
        let weak = serde_json::json!({
            "device_id": "phone-2",
            "public_key": public,
            "nonce": nonce,
            "sig": only_nonce,
        })
        .to_string();
        let (status, raw) = http(port, "POST", "/v1/pair", &weak).await;
        assert_eq!(status, 403);
        assert!(raw.contains("bad_signature"));
        let local_challenge = fresh_challenge(port).await;
        let local_sig = phone.sign_b64(&pairing::revoke_message(&phone.id, &local_challenge));
        let (status, _) = http_headers(
            port,
            "POST",
            &format!("/v1/devices/{}/revoke", phone.id),
            "",
            &format!("127.0.0.1:{port}"),
            &[
                ("x-pendant-device", phone.id.as_str()),
                ("x-pendant-signature", local_sig.as_str()),
                ("x-pendant-challenge", local_challenge.as_str()),
            ],
        )
        .await;
        assert_eq!(status, 200);
        let (status, raw) = http(port, "GET", "/v1/pair/qr", "").await;
        assert_eq!(status, 200);
        let nonce = response_json(&raw)["nonce"].as_str().unwrap().to_string();
        let again = phone.sign_b64(&pairing::pair_message(&phone.id, &public, &nonce));
        let body = serde_json::json!({
            "device_id": phone.id,
            "public_key": public,
            "nonce": nonce,
            "sig": again,
        })
        .to_string();
        let (status, raw) = http(port, "POST", "/v1/pair", &body).await;
        assert_eq!(status, 403);
        assert!(raw.contains("revoked"));
        let _ = status;
    }
}
