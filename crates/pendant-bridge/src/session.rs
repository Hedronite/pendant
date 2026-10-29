//! One phone WebSocket. Envelope version is 1, multiplexed by pane id.
//!
//! `input`, `resize`, and `scroll` are refused. An optional `sig` is ignored.

use std::collections::{HashMap, VecDeque};

use axum::extract::ws::{Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use pendant_herdr::HerdrClient;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::mpsc;

use crate::AppState;

pub const ENVELOPE_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WireFrame {
    pub full: bool,
    pub seq: u64,
    pub bytes: String,
    pub width: u32,
    pub height: u32,
}

/// Per-phone frame queue. A seq gap or a flooded queue yields one `full` frame.
#[derive(Debug)]
pub struct PhoneQueue {
    cap: usize,
    items: VecDeque<WireFrame>,
    need_full: bool,
    last_full: Option<WireFrame>,
    expect: Option<u64>,
}

impl PhoneQueue {
    pub fn new(cap: usize) -> Self {
        Self {
            cap: cap.max(1),
            items: VecDeque::new(),
            need_full: false,
            last_full: None,
            expect: None,
        }
    }

    pub fn push_observed(&mut self, frame: WireFrame) {
        let gap = self
            .expect
            .is_some_and(|prev| frame.seq > prev.saturating_add(1));
        self.expect = Some(frame.seq);
        if frame.full {
            self.last_full = Some(frame.clone());
        }
        if gap {
            if let Some(full) = self.last_full.clone() {
                self.enqueue(full);
            }
            return;
        }
        self.enqueue(frame);
    }

    fn enqueue(&mut self, frame: WireFrame) {
        if self.items.len() >= self.cap {
            if !frame.full {
                self.need_full = true;
                return;
            }
            self.items.clear();
            self.need_full = false;
        } else if self.need_full && !frame.full {
            return;
        }
        if frame.full {
            self.need_full = false;
        }
        self.items.push_back(frame);
    }

    pub fn pop(&mut self) -> Option<WireFrame> {
        let item = self.items.pop_front()?;
        if self.items.is_empty() && self.need_full {
            self.need_full = false;
            if let Some(full) = self.last_full.clone() {
                self.items.push_back(full);
            }
        }
        Some(item)
    }
}

#[derive(Debug, Deserialize)]
struct Up {
    v: u32,
    pane: String,
    #[serde(rename = "type")]
    kind: String,
    cols: Option<u16>,
    rows: Option<u16>,
    /// Accepted and ignored. Reserved verbs stay refused.
    #[serde(default)]
    sig: Option<String>,
}

pub async fn run_socket(socket: WebSocket, state: AppState) {
    let (mut sink, mut stream) = socket.split();
    let (tx, mut rx) = mpsc::channel::<String>(64);
    let writer = tokio::spawn(async move {
        while let Some(text) = rx.recv().await {
            if sink.send(Message::Text(text.into())).await.is_err() {
                break;
            }
        }
    });

    let mut watches: HashMap<String, tokio::task::AbortHandle> = HashMap::new();
    let fulls: std::sync::Arc<std::sync::Mutex<HashMap<String, WireFrame>>> =
        std::sync::Arc::new(std::sync::Mutex::new(HashMap::new()));

    while let Some(Ok(msg)) = stream.next().await {
        let Message::Text(text) = msg else {
            continue;
        };
        let up: Up = match serde_json::from_str(text.as_str()) {
            Ok(up) => up,
            Err(_) => continue,
        };
        if up.v != ENVELOPE_VERSION {
            continue;
        }
        let _ignored_sig = up.sig;
        match up.kind.as_str() {
            "watch" => {
                let cols = up.cols.unwrap_or(80);
                let rows = up.rows.unwrap_or(24);
                if let Some(prev) = watches.remove(&up.pane) {
                    prev.abort();
                }
                send_scrollback(&state.herdr, &tx, &up.pane).await;
                let handle = spawn_watch(
                    state.herdr.clone(),
                    tx.clone(),
                    up.pane.clone(),
                    cols,
                    rows,
                    state.queue_cap,
                    fulls.clone(),
                );
                watches.insert(up.pane, handle);
            }
            "unwatch" => {
                if let Some(prev) = watches.remove(&up.pane) {
                    prev.abort();
                }
            }
            "resync" => {
                if let Some(full) = fulls.lock().ok().and_then(|map| map.get(&up.pane).cloned()) {
                    let _ = tx.send(frame_json(&up.pane, &full)).await;
                }
            }
            "input" | "resize" | "scroll" => {
                let _ = tx
                    .send(
                        json!({
                            "v": ENVELOPE_VERSION,
                            "pane": up.pane,
                            "type": "refused",
                            "op": up.kind,
                        })
                        .to_string(),
                    )
                    .await;
            }
            _ => {}
        }
    }
    for handle in watches.into_values() {
        handle.abort();
    }
    writer.abort();
}

fn spawn_watch(
    herdr: HerdrClient,
    tx: mpsc::Sender<String>,
    pane: String,
    cols: u16,
    rows: u16,
    cap: usize,
    fulls: std::sync::Arc<std::sync::Mutex<HashMap<String, WireFrame>>>,
) -> tokio::task::AbortHandle {
    let task = tokio::spawn(async move {
        let mut obs = match herdr.observe(&pane, cols, rows).await {
            Ok(obs) => obs,
            Err(_) => return,
        };
        let mut queue = PhoneQueue::new(cap);
        loop {
            let frame = match obs.next_frame().await {
                Ok(frame) => frame,
                Err(_) => break,
            };
            queue.push_observed(WireFrame {
                full: frame.full,
                seq: frame.seq,
                bytes: frame.bytes_b64,
                width: frame.width,
                height: frame.height,
            });
            while let Some(out) = queue.pop() {
                if out.full {
                    if let Ok(mut map) = fulls.lock() {
                        map.insert(pane.clone(), out.clone());
                    }
                }
                if tx.send(frame_json(&pane, &out)).await.is_err() {
                    return;
                }
            }
        }
    });
    task.abort_handle()
}

fn frame_json(pane: &str, frame: &WireFrame) -> String {
    json!({
        "v": ENVELOPE_VERSION,
        "pane": pane,
        "type": "frame",
        "full": frame.full,
        "seq": frame.seq,
        "bytes": frame.bytes,
        "width": frame.width,
        "height": frame.height,
    })
    .to_string()
}

async fn send_scrollback(herdr: &HerdrClient, tx: &mpsc::Sender<String>, pane: &str) {
    let value = match herdr
        .rpc(
            "pane.read",
            json!({
                "pane_id": pane,
                "source": "recent",
                "format": "text",
                "strip_ansi": false
            }),
        )
        .await
    {
        Ok(value) => value,
        Err(_) => return,
    };
    let text = scroll_text(&value);
    let _ = tx
        .send(
            json!({
                "v": ENVELOPE_VERSION,
                "pane": pane,
                "type": "scrollback",
                "text": text,
            })
            .to_string(),
        )
        .await;
}

fn scroll_text(value: &Value) -> String {
    value
        .pointer("/read/text")
        .or_else(|| value.get("text"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

/// Writes are allowed only for `…/.config/herdr/sessions/<name>/herdr.sock`.
pub fn scratch_socket(path: &std::path::Path) -> bool {
    let parts: Vec<&str> = path
        .components()
        .filter_map(|c| c.as_os_str().to_str())
        .collect();
    let n = parts.len();
    n >= 5
        && parts[n - 1] == "herdr.sock"
        && parts[n - 3] == "sessions"
        && parts[n - 4] == "herdr"
        && parts[n - 5] == ".config"
        && !parts[n - 2].is_empty()
}

pub fn approve_key(key: &str) -> bool {
    const BAR: &[&str] = &[
        "y", "n", "1", "2", "3", "4", "enter", "esc", "tab", "space", "up", "down", "left",
        "right", "ctrl+c", "ctrl+d", "ctrl+z",
    ];
    let key = key.trim().to_ascii_lowercase().replace("ctrl-", "ctrl+");
    BAR.contains(&key.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full(seq: u64) -> WireFrame {
        WireFrame {
            full: true,
            seq,
            bytes: "YQ==".into(),
            width: 40,
            height: 12,
        }
    }

    fn incr(seq: u64) -> WireFrame {
        WireFrame {
            full: false,
            seq,
            bytes: "Yg==".into(),
            width: 40,
            height: 12,
        }
    }

    #[test]
    fn seq_gap_yields_one_full() {
        let mut queue = PhoneQueue::new(8);
        queue.push_observed(full(1));
        let first = queue.pop().unwrap();
        queue.push_observed(incr(3));
        let recovery = queue.pop().unwrap();
        assert!(first.full);
        assert_eq!(first.seq, 1);
        assert!(recovery.full);
        assert!(queue.pop().is_none());
    }

    #[test]
    fn flooded_queue_drains_one_full() {
        let mut queue = PhoneQueue::new(1);
        queue.push_observed(full(1));
        queue.push_observed(incr(2));
        queue.push_observed(incr(3));
        let first = queue.pop().unwrap();
        let drained = queue.pop().unwrap();
        assert!(first.full);
        assert!(drained.full);
        assert!(queue.pop().is_none());
    }

    #[test]
    fn scratch_socket_is_a_named_session_path() {
        let ok = std::path::Path::new("/tmp/work/.config/herdr/sessions/g1/herdr.sock");
        let live = std::path::Path::new("/tmp/work/.config/herdr/herdr.sock");
        assert!(scratch_socket(ok));
        assert!(!scratch_socket(live));
    }

    #[test]
    fn approve_bar_rejects_other_keys() {
        assert!(approve_key("y"));
        assert!(approve_key("ctrl+c"));
        assert!(approve_key("Ctrl-d"));
        assert!(!approve_key("rm"));
        assert!(!approve_key("a"));
    }
}

#[cfg(test)]
mod dial {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::time::Duration;

    use futures_util::{SinkExt, StreamExt};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::{TcpListener, UnixListener};
    use tokio_tungstenite::connect_async;

    const MARKER: &str = "PENDANT-BRIDGE-G1-MARKER";

    /// FAKE herdr binary. This test does not spawn the real herdr.
    #[tokio::test]
    async fn websocket_dials_loopback_and_speaks_the_envelope() {
        let dir = tempfile::tempdir().unwrap();
        let sock_path = dir
            .path()
            .join(".config/herdr/sessions/g1/herdr.sock");
        std::fs::create_dir_all(sock_path.parent().unwrap()).unwrap();
        let _ = std::fs::remove_file(&sock_path);
        let listener = UnixListener::bind(&sock_path).unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                tokio::spawn(async move {
                    let mut reader = BufReader::new(stream);
                    let mut line = String::new();
                    if reader.read_line(&mut line).await.unwrap_or(0) == 0 {
                        return;
                    }
                    let req: Value = serde_json::from_str(line.trim()).unwrap_or(Value::Null);
                    let id = req.get("id").and_then(Value::as_str).unwrap_or("");
                    let method = req.get("method").and_then(Value::as_str).unwrap_or("");
                    let result = if method == "pane.read" {
                        json!({"text": format!("seed {MARKER}")})
                    } else if method == "events.subscribe" {
                        json!({"type": "subscription_started"})
                    } else {
                        json!({"ok": true})
                    };
                    let resp = json!({"id": id, "result": result});
                    let mut stream = reader.into_inner();
                    let _ = stream
                        .write_all(format!("{resp}\n").as_bytes())
                        .await;
                    if method == "events.subscribe" {
                        tokio::time::sleep(Duration::from_secs(30)).await;
                    }
                });
            }
        });

        let script = dir.path().join("fake-herdr");
        std::fs::write(
            &script,
            "#!/bin/sh\nprintf '%s\\n' '{\"type\":\"terminal.frame\",\"seq\":1,\"encoding\":\"ansi\",\"width\":40,\"height\":12,\"full\":true,\"bytes\":\"YQ==\"}'\nprintf '%s\\n' '{\"type\":\"terminal.frame\",\"seq\":2,\"encoding\":\"ansi\",\"width\":40,\"height\":12,\"full\":false,\"bytes\":\"Yg==\"}'\nsleep 30\n",
        )
        .unwrap();
        let mut perms = std::fs::metadata(&script).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&script, perms).unwrap();

        let state = crate::AppState {
            herdr: HerdrClient::with_herdr_bin(&sock_path, &script),
            queue_cap: 8,
        };
        let tcp = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = tcp.local_addr().unwrap().port();
        tokio::spawn(async move {
            let _ = axum::serve(tcp, crate::app(state)).await;
        });

        let url = format!("ws://127.0.0.1:{port}/v1/ws");
        let (mut ws, _) = connect_async(&url).await.unwrap();
        ws.send(tokio_tungstenite::tungstenite::Message::Text(
            r#"{"v":1,"pane":"w1:p1","type":"watch","cols":40,"rows":12}"#.into(),
        ))
        .await
        .unwrap();

        let mut saw_marker = false;
        let mut seqs = Vec::new();
        for _ in 0..6 {
            let msg = tokio::time::timeout(Duration::from_secs(2), ws.next())
                .await
                .expect("timed out waiting for a frame")
                .unwrap()
                .unwrap();
            let text = msg.into_text().unwrap();
            let body: Value = serde_json::from_str(&text).unwrap();
            if body["type"] == "scrollback" && body["text"].as_str().unwrap_or("").contains(MARKER)
            {
                saw_marker = true;
            }
            if body["type"] == "frame" {
                seqs.push(body["seq"].as_u64().unwrap());
                if seqs.len() == 2 {
                    break;
                }
            }
        }
        assert!(saw_marker, "scrollback missing the marker");
        assert!(seqs.len() >= 2 && seqs[1] > seqs[0]);

        for op in ["input", "resize", "scroll"] {
            let payload = format!(r#"{{"v":1,"pane":"w1:p1","type":"{op}","sig":"ignored"}}"#);
            ws.send(tokio_tungstenite::tungstenite::Message::Text(payload.into()))
                .await
                .unwrap();
            let msg = tokio::time::timeout(Duration::from_secs(2), ws.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            let body: Value = serde_json::from_str(&msg.into_text().unwrap()).unwrap();
            assert_eq!(body["type"], "refused");
            assert_eq!(body["op"], op);
        }
        drop(ws);

        let (mut again, _) = connect_async(&url).await.unwrap();
        again
            .send(tokio_tungstenite::tungstenite::Message::Text(
                r#"{"v":1,"pane":"w1:p1","type":"watch","cols":40,"rows":12}"#.into(),
            ))
            .await
            .unwrap();
        let mut reconnect_full = false;
        for _ in 0..4 {
            let msg = tokio::time::timeout(Duration::from_secs(2), again.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            let body: Value = serde_json::from_str(&msg.into_text().unwrap()).unwrap();
            if body["type"] == "frame" {
                assert!(body["full"].as_bool().unwrap());
                reconnect_full = true;
                break;
            }
        }
        assert!(reconnect_full, "reconnect did not start with one full frame");
    }
}
