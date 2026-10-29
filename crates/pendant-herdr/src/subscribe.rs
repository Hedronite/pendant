use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::unix::OwnedReadHalf;

use crate::error::HerdrError;
use crate::protocol::{RpcRequest, RpcResponse};
use crate::via::{stamp, Via};
use crate::HerdrClient;

/// Open `events.subscribe` stream. The socket stays open until this is dropped.
pub struct Subscription {
    pub ack: Value,
    pub events: EventStream,
}

/// Lines after the `subscription_started` acknowledgement. Reading blocks.
/// There is no poll interval.
pub struct EventStream {
    reader: BufReader<OwnedReadHalf>,
    _keep: tokio::net::unix::OwnedWriteHalf,
}

impl EventStream {
    /// Next event from the same connection. `via` is `socket`.
    pub async fn next_event(&mut self) -> Result<Value, HerdrError> {
        let mut line = String::new();
        let n = self.reader.read_line(&mut line).await?;
        if n == 0 {
            return Err(HerdrError::EmptyResponse);
        }
        let parsed: Value = serde_json::from_str(line.trim())?;
        Ok(stamp(parsed, Via::Socket))
    }
}

impl HerdrClient {
    /// Subscribe and leave the socket open. Does not fall back to the CLI:
    /// a one-shot CLI command cannot carry the stream, and a connect failure
    /// here is an error.
    pub async fn subscribe_events(&self, subscriptions: &[Value]) -> Result<Subscription, HerdrError> {
        let id = self.alloc_id();
        let request = RpcRequest::new(
            id.clone(),
            "events.subscribe",
            json!({ "subscriptions": subscriptions }),
        );
        let line = request.to_line()?;
        let stream = self.connect_socket().await?;
        let (read, mut write) = stream.into_split();
        write.write_all(line.as_bytes()).await?;
        write.flush().await?;

        let mut reader = BufReader::new(read);
        let mut ack_line = String::new();
        let n = reader.read_line(&mut ack_line).await?;
        if n == 0 {
            return Err(HerdrError::EmptyResponse);
        }
        let parsed: RpcResponse = serde_json::from_str(ack_line.trim())?;
        if let Some(reply_id) = parsed.id.as_deref() {
            if !reply_id.is_empty() && reply_id != id && !reply_id.starts_with(&format!("{id}:sub:"))
            {
                return Err(HerdrError::Rpc {
                    code: "bad_id".into(),
                    message: format!("events.subscribe reply id {reply_id}"),
                    body: None,
                });
            }
        }
        if let Some(err) = parsed.error {
            return Err(HerdrError::from_rpc(err));
        }
        let result = parsed.result.unwrap_or(Value::Null);
        let ack_type = result.get("type").and_then(Value::as_str);
        if ack_type != Some("subscription_started") {
            return Err(HerdrError::Rpc {
                code: "bad_ack".into(),
                message: format!("events.subscribe ack was {result}"),
                body: None,
            });
        }
        Ok(Subscription {
            ack: stamp(result, Via::Socket),
            events: EventStream {
                reader,
                _keep: write,
            },
        })
    }
}

/// First `revision` number on this value, depth-first.
pub fn revision_of(value: &Value) -> Option<u64> {
    match value {
        Value::Object(map) => {
            if let Some(n) = map.get("revision").and_then(Value::as_u64) {
                return Some(n);
            }
            for child in map.values() {
                if let Some(n) = revision_of(child) {
                    return Some(n);
                }
            }
            None
        }
        Value::Array(items) => items.iter().find_map(revision_of),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    use tokio::net::UnixListener;

    /// Polling is off: one connection, no second request, the event arrives on that socket.
    #[tokio::test]
    async fn subscribe_stays_open_and_yields_revision_without_polling() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("herdr.sock");
        let listener = UnixListener::bind(&sock).unwrap();
        let accepts = Arc::new(AtomicUsize::new(0));
        let accepts_bg = accepts.clone();

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            accepts_bg.fetch_add(1, Ordering::SeqCst);
            let (read, mut write) = stream.into_split();
            let mut reader = BufReader::new(read);
            let mut line = String::new();
            reader.read_line(&mut line).await.unwrap();
            let req: Value = serde_json::from_str(line.trim()).unwrap();
            assert_eq!(req["method"], "events.subscribe");
            let id = req["id"].as_str().unwrap().to_string();

            line.clear();
            let second = tokio::time::timeout(Duration::from_millis(40), reader.read_line(&mut line)).await;
            assert!(second.is_err(), "client wrote again before the ack; that is a poll");

            let ack = json!({
                "id": id,
                "result": { "type": "subscription_started" }
            });
            write.write_all(format!("{ack}\n").as_bytes()).await.unwrap();
            write.flush().await.unwrap();

            line.clear();
            let after_ack =
                tokio::time::timeout(Duration::from_millis(40), reader.read_line(&mut line)).await;
            assert!(after_ack.is_err(), "client wrote again after the ack; that is a poll");

            let event = json!({
                "event": "pane.agent_status_changed",
                "data": {
                    "pane_id": "w1:p1",
                    "workspace_id": "w1",
                    "agent_status": "working",
                    "revision": 7
                }
            });
            write.write_all(format!("{event}\n").as_bytes()).await.unwrap();
            write.flush().await.unwrap();
            tokio::time::sleep(Duration::from_millis(200)).await;
        });

        let client = HerdrClient::new(&sock);
        let mut sub = client
            .subscribe_events(&[json!({ "type": "pane.agent_status_changed" })])
            .await
            .unwrap();
        assert_eq!(sub.ack["via"], "socket");
        assert_eq!(sub.ack["type"], "subscription_started");
        let event = tokio::time::timeout(Duration::from_secs(2), sub.events.next_event())
            .await
            .expect("event timed out")
            .unwrap();
        assert_eq!(event["via"], "socket");
        assert_eq!(event["event"], "pane.agent_status_changed");
        assert_eq!(event["data"]["agent_status"], "working");
        assert_eq!(revision_of(&event), Some(7));
        assert_eq!(accepts.load(Ordering::SeqCst), 1);
        server.await.unwrap();
    }
}
