use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::cli;
use crate::error::HerdrError;
use crate::paths::expand_path;
use crate::protocol::{RpcRequest, RpcResponse};
use crate::snapshot::{InboxItem, Snapshot};

const RPC_TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Clone, Debug)]
pub struct HerdrClient {
    socket_path: PathBuf,
    next_id: Arc<AtomicU64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct CreateWorktree {
    #[serde(default)]
    pub workspace_id: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub base: Option<String>,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub focus: bool,
}

impl HerdrClient {
    pub fn new(socket_path: impl AsRef<Path>) -> Self {
        Self {
            socket_path: socket_path.as_ref().to_path_buf(),
            next_id: Arc::new(AtomicU64::new(1)),
        }
    }

    pub fn from_env_path(raw: &str) -> Self {
        Self::new(expand_path(raw))
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    pub async fn snapshot(&self) -> Result<Snapshot, HerdrError> {
        let raw = self.rpc("session.snapshot", json!({})).await?;
        Ok(Snapshot::from_raw(raw))
    }

    pub async fn inbox(&self) -> Result<Vec<InboxItem>, HerdrError> {
        Ok(self.snapshot().await?.inbox())
    }

    pub async fn worktree_list(&self, workspace_id: Option<&str>) -> Result<Value, HerdrError> {
        let mut params = json!({});
        if let Some(id) = workspace_id {
            params["workspace_id"] = json!(id);
        }
        self.rpc("worktree.list", params).await
    }

    pub async fn worktree_create(&self, create: &CreateWorktree) -> Result<Value, HerdrError> {
        self.rpc("worktree.create", serde_json::to_value(create)?)
            .await
    }

    pub async fn agent_prompt(&self, target: &str, text: &str) -> Result<Value, HerdrError> {
        self.rpc("agent.prompt", json!({ "target": target, "text": text }))
            .await
    }

    pub async fn agent_send_keys(
        &self,
        target: &str,
        keys: &[String],
    ) -> Result<Value, HerdrError> {
        self.rpc("agent.send_keys", json!({ "target": target, "keys": keys }))
            .await
    }

    pub async fn agent_read(&self, target: &str) -> Result<Value, HerdrError> {
        self.rpc(
            "agent.read",
            json!({ "target": target, "source": "recent", "lines": 80 }),
        )
        .await
    }

    pub async fn agent_get(&self, target: &str) -> Result<Value, HerdrError> {
        self.rpc("agent.get", json!({ "target": target })).await
    }

    pub async fn ping(&self) -> Result<Value, HerdrError> {
        self.rpc("ping", json!({})).await
    }

    pub async fn health(&self) -> bool {
        match tokio::time::timeout(Duration::from_secs(2), self.ping()).await {
            Ok(Ok(_)) => true,
            _ => self.snapshot().await.is_ok(),
        }
    }

    /// One-shot `events.subscribe` read of the acknowledgement line.
    /// Long-lived streaming is owned by the bridge SSE loop (poll fallback).
    pub async fn subscribe_ack(&self) -> Result<Value, HerdrError> {
        self.rpc(
            "events.subscribe",
            json!({
                "subscriptions": [
                    { "type": "pane.agent_status_changed" }
                ]
            }),
        )
        .await
    }

    pub async fn rpc(&self, method: &str, params: Value) -> Result<Value, HerdrError> {
        match self.rpc_socket(method, params.clone()).await {
            Ok(value) => Ok(value),
            Err(sock_err) if sock_err.is_transport() => {
                tracing::debug!(error = %sock_err, method, "herdr socket unavailable; trying CLI");
                match cli::rpc_cli(&self.socket_path, method, params).await {
                    Ok(value) => Ok(value),
                    Err(HerdrError::UnsupportedMethod(method)) => {
                        Err(HerdrError::UnsupportedMethod(method))
                    }
                    Err(cli_err) => Err(HerdrError::Unavailable {
                        path: self.socket_path.clone(),
                        socket: sock_err.to_string(),
                        cli: cli_err.to_string(),
                    }),
                }
            }
            Err(err) => Err(err),
        }
    }

    async fn rpc_socket(&self, method: &str, params: Value) -> Result<Value, HerdrError> {
        let id = format!("p-{}", self.next_id.fetch_add(1, Ordering::Relaxed));
        let request = RpcRequest::new(id.clone(), method, params);
        let line = request.to_line()?;

        let connect = tokio::net::UnixStream::connect(&self.socket_path);
        let stream = match tokio::time::timeout(RPC_TIMEOUT, connect).await {
            Ok(Ok(stream)) => stream,
            Ok(Err(source)) => {
                return Err(HerdrError::Connect {
                    path: self.socket_path.clone(),
                    source,
                });
            }
            Err(_) => return Err(HerdrError::Timeout),
        };

        let exchange = async {
            let mut stream = stream;
            stream.write_all(line.as_bytes()).await?;
            stream.flush().await?;
            let mut reader = BufReader::new(stream);
            let mut response_line = String::new();
            let n = reader.read_line(&mut response_line).await?;
            if n == 0 {
                return Err(HerdrError::EmptyResponse);
            }
            let parsed: RpcResponse = serde_json::from_str(response_line.trim())?;
            if let Some(err) = parsed.error {
                return Err(HerdrError::from_rpc(err));
            }
            Ok(parsed.result.unwrap_or(Value::Null))
        };

        match tokio::time::timeout(RPC_TIMEOUT, exchange).await {
            Ok(result) => result,
            Err(_) => Err(HerdrError::Timeout),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixListener;

    #[tokio::test]
    async fn socket_snapshot_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("herdr.sock");
        let listener = UnixListener::bind(&sock).unwrap();

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut reader = BufReader::new(stream);
            let mut line = String::new();
            reader.read_line(&mut line).await.unwrap();
            let req: Value = serde_json::from_str(&line).unwrap();
            assert_eq!(req["method"], "session.snapshot");
            assert!(req["id"].as_str().unwrap().starts_with("p-"));
            let resp = json!({
                "id": req["id"],
                "result": {
                    "type": "session_snapshot",
                    "agents": [{
                        "pane_id": "w1:p1",
                        "agent": "codex",
                        "agent_status": "idle",
                        "workspace_id": "w1",
                        "tab_id": "w1:t1"
                    }]
                }
            });
            let mut stream = reader.into_inner();
            stream
                .write_all(format!("{resp}\n").as_bytes())
                .await
                .unwrap();
        });

        let client = HerdrClient::new(&sock);
        let snap = client.snapshot().await.unwrap();
        assert_eq!(snap.agents.len(), 1);
        assert_eq!(snap.agents[0].pane_id, "w1:p1");
        assert_eq!(snap.agents[0].status, "idle");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn missing_socket_is_transport_error() {
        let client = HerdrClient::new("/tmp/pendant-definitely-missing.sock");
        let err = client.rpc_socket("ping", json!({})).await.unwrap_err();
        assert!(err.is_transport());
    }

    #[tokio::test]
    async fn missing_socket_and_cli_is_unavailable() {
        let client = HerdrClient::new("/tmp/pendant-definitely-missing.sock");
        let err = client.rpc("ping", json!({})).await.unwrap_err();
        assert!(matches!(err, HerdrError::Unavailable { .. }));
    }
}
