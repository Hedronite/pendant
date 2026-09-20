//! Client for a local [herdr](https://herdr.dev) 0.9.x server.
//!
//! Prefers newline-delimited JSON over the Unix domain socket. If the socket
//! cannot be reached, falls back to `herdr` CLI JSON (`herdr api snapshot`,
//! `herdr worktree …`, `herdr agent …`).

mod cli;
mod client;
mod error;
mod paths;
mod protocol;
mod snapshot;

pub use client::{CreateWorktree, HerdrClient};
pub use error::HerdrError;
pub use paths::{default_socket_path, expand_path, home_dir, DEFAULT_SOCKET_PATH};
pub use protocol::{RpcError, RpcRequest, RpcResponse};
pub use snapshot::{AgentRecord, InboxItem, Snapshot, WorkspaceRecord};
