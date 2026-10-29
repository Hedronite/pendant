//! Loopback HTTP bridge. Never bind `0.0.0.0` / `::`.

mod bind;
mod config;
mod error;
mod routes;
mod session;
mod static_files;

pub use bind::{parse_bind, BindError, DEFAULT_BIND};
pub use config::Config;
pub use error::BridgeError;

use std::net::SocketAddr;

use axum::Router;
use pendant_herdr::HerdrClient;
use tokio::net::TcpListener;
use tower_http::trace::TraceLayer;

#[derive(Clone)]
pub struct AppState {
    pub herdr: HerdrClient,
    pub queue_cap: usize,
}

pub fn app(state: AppState) -> Router {
    routes::router(state).layer(TraceLayer::new_for_http())
}

pub async fn serve(config: Config) -> Result<(), BridgeError> {
    if config.bind.ip().is_unspecified() {
        return Err(BridgeError::UnspecifiedBind(config.bind));
    }

    let state = AppState {
        herdr: HerdrClient::new(&config.herdr_sock),
        queue_cap: 32,
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
