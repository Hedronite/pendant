//! Loopback HTTP bridge. Never bind a non-loopback address.

mod bind;
mod config;
mod error;
mod pairing;
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
    /// Loopback address this process bound. Used for Host and Origin checks.
    pub bind: SocketAddr,
    /// Optional Serve origin, such as `https://name.example`. Not a hard-coded name.
    pub serve_origin: Option<String>,
    pub pairing: pairing::Pairing,
}

pub fn app(state: AppState) -> Router {
    routes::router(state).layer(TraceLayer::new_for_http())
}

pub async fn serve(config: Config) -> Result<(), BridgeError> {
    assert_loopback_only(config.bind)?;

    let listener = TcpListener::bind(config.bind).await?;
    let local = listener.local_addr()?;
    assert_loopback_only(local)?;

    let state = AppState {
        herdr: HerdrClient::new(&config.herdr_sock),
        queue_cap: 32,
        bind: local,
        serve_origin: std::env::var("PENDANT_SERVE_ORIGIN")
            .ok()
            .filter(|value| !value.is_empty()),
        pairing: pairing::Pairing::new(),
    };

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
    } else if !addr.ip().is_loopback() {
        Err(BridgeError::NonLoopbackBind(addr))
    } else {
        Ok(())
    }
}
