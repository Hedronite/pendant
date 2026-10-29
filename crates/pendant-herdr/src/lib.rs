//! Client for a local [herdr](https://herdr.dev) 0.9.x server.
//!
//! NDJSON over the Unix socket is the default. Every RPC reply carries
//! `via: socket` or `via: cli`. The CLI is the labelled fallback, and only
//! when connecting to an existing socket inode fails. A missing or non-socket
//! path returns the connect error and does not call `rpc_cli`.
//!
//! `events.subscribe` stays open ([`HerdrClient::subscribe_events`]).
//! `herdr terminal session observe` runs as a stdio child
//! ([`HerdrClient::observe`]).

mod cli;
mod client;
mod error;
mod observe;
mod paths;
mod protocol;
mod snapshot;
mod subscribe;
mod via;

pub use client::{CreateWorktree, HerdrClient};
pub use error::HerdrError;
pub use observe::{ObserveSession, TerminalFrame};
pub use paths::{
    default_socket_path, expand_path, home_dir, named_session, DEFAULT_SOCKET_PATH,
};
pub use protocol::{RpcError, RpcRequest, RpcResponse};
pub use snapshot::{AgentRecord, InboxItem, Snapshot, WorkspaceRecord};
pub use subscribe::{EventStream, Subscription};
pub use via::Via;
