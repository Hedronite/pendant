use std::path::PathBuf;

use thiserror::Error;

use crate::protocol::RpcError;

#[derive(Debug, Error)]
pub enum HerdrError {
    #[error("herdr socket not reachable at {path}: {source}")]
    Connect {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("herdr socket I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("timed out talking to herdr")]
    Timeout,
    #[error("herdr returned no line of JSON")]
    EmptyResponse,
    #[error("invalid JSON from herdr: {0}")]
    Json(#[from] serde_json::Error),
    #[error("herdr RPC error {code}: {message}")]
    Rpc {
        code: String,
        message: String,
        body: Option<RpcError>,
    },
    #[error("herdr line exceeded {limit} bytes")]
    LineTooLong { limit: usize },
    #[error("herdr line is not utf-8")]
    InvalidLine,
    #[error("observe target must not start with '-'")]
    BadObserveTarget,
    #[error("herdr CLI not found on PATH")]
    CliMissing,
    #[error("herdr CLI failed ({status}): {stderr}")]
    CliFailed { status: i32, stderr: String },
    #[error("no CLI mapping for herdr method {0}")]
    UnsupportedMethod(String),
    #[error("herdr unavailable at {path}: {socket}; CLI fallback: {cli}")]
    Unavailable {
        path: PathBuf,
        socket: String,
        cli: String,
    },
}

impl HerdrError {
    pub fn is_transport(&self) -> bool {
        matches!(
            self,
            Self::Connect { .. }
                | Self::Io(_)
                | Self::Timeout
                | Self::CliMissing
                | Self::EmptyResponse
        )
    }

    pub fn from_rpc(err: RpcError) -> Self {
        Self::Rpc {
            code: err.code.clone(),
            message: err.message.clone(),
            body: Some(err),
        }
    }
}
