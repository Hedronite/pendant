//! Loopback HTTP bridge. Never bind `0.0.0.0` / `::`.

mod bind;
mod config;
mod error;
mod routes;
mod static_files;

pub use bind::{parse_bind, BindError, DEFAULT_BIND};
pub use config::Config;
pub use error::BridgeError;

use std::net::SocketAddr;

use axum::extract::{Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use pendant_herdr::HerdrClient;
use serde_json::json;
use tokio::net::TcpListener;
use tower_http::trace::TraceLayer;

/// JSON bodies are small approve/prompt payloads; cap like the prototype.
const MAX_BODY_BYTES: usize = 64 * 1024;

#[derive(Clone)]
pub struct AppState {
    pub herdr: HerdrClient,
    pub token: Option<String>,
}

pub fn app(state: AppState) -> Router {
    routes::router(state.clone())
        .route_layer(middleware::from_fn_with_state(state, bearer_auth))
        .layer(TraceLayer::new_for_http())
        .layer(axum::extract::DefaultBodyLimit::max(MAX_BODY_BYTES))
}

/// When `PENDANT_TOKEN` is set, every `/v1/*` call needs
/// `Authorization: Bearer <token>`. Unset = Tailscale mesh is the boundary.
/// Static PWA assets stay open (they carry no data and hold no secrets).
async fn bearer_auth(State(state): State<AppState>, req: Request, next: Next) -> Response {
    if let Some(token) = &state.token {
        let ok = req
            .headers()
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v == format!("Bearer {token}"));
        if !ok {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({"error":{"code":"unauthorized","message":"bearer token required"}})),
            )
                .into_response();
        }
    }
    next.run(req).await
}

pub async fn serve(config: Config) -> Result<(), BridgeError> {
    if config.bind.ip().is_unspecified() {
        return Err(BridgeError::UnspecifiedBind(config.bind));
    }

    let state = AppState {
        herdr: HerdrClient::new(&config.herdr_sock),
        token: config.token.clone(),
    };
    let listener = TcpListener::bind(config.bind).await?;
    let local = listener.local_addr()?;
    if local.ip().is_unspecified() {
        return Err(BridgeError::UnspecifiedBind(local));
    }

    tracing::info!(
        %local,
        sock = %config.herdr_sock.display(),
        "pendant listening (loopback only)"
    );

    axum::serve(listener, app(state))
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}

/// Bind-time check used by tests and the binary.
pub fn assert_loopback_only(addr: SocketAddr) -> Result<(), BridgeError> {
    if addr.ip().is_unspecified() {
        Err(BridgeError::UnspecifiedBind(addr))
    } else {
        Ok(())
    }
}
