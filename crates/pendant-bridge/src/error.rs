use std::net::SocketAddr;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum BridgeError {
    #[error("invalid PENDANT_BIND {input:?}: {message}")]
    InvalidBind { input: String, message: String },
    #[error("refusing unspecified bind address {0} (no 0.0.0.0 / ::)")]
    UnspecifiedBind(SocketAddr),
    #[error("listen failed: {0}")]
    Listen(#[from] std::io::Error),
}
