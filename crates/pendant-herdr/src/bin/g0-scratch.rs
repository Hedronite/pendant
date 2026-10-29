//! Scratch-session check for Pendant bridge G0.
//!
//! Runs against `~/.config/herdr/sessions/<name>/herdr.sock` only.
//! Spawns the `herdr` on `PATH` (0.9.5). Does not build.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use pendant_herdr::{named_session, HerdrClient};
use serde_json::{json, Value};
use tokio::process::Command;
use tokio::time::timeout;

const MARKER: &str = "PENDANT-BRIDGE-G0-MARKER-7f3a";
const INCR: &str = "PENDANT-BRIDGE-G0-INCR-7f3a";

#[tokio::main]
async fn main() {
    let result = run().await;
    match result {
        Ok(report) => {
            let text = serde_json::to_string_pretty(&report).unwrap_or_else(|_| "{}".into());
            println!("{text}");
            if let Ok(path) = std::env::var("PENDANT_G0_EVIDENCE") {
                if let Err(err) = std::fs::write(&path, text) {
                    eprintln!("could not write {path}: {err}");
                    std::process::exit(1);
                }
            }
            if report.get("ok").and_then(Value::as_bool) != Some(true) {
                std::process::exit(1);
            }
        }
        Err(err) => {
            eprintln!("g0 scratch failed: {err}");
            std::process::exit(1);
        }
    }
}

async fn run() -> Result<Value, String> {
    let raw = std::env::var("PENDANT_HERDR_SOCK").map_err(|_| "PENDANT_HERDR_SOCK is not set")?;
    let socket = pendant_herdr::expand_path(&raw);
    assert_scratch(&socket)?;
    let session = named_session(&socket).ok_or("named session missing")?;

    let version = herdr_version().await?;
    let client = HerdrClient::new(&socket);
    let snap = client
        .snapshot()
        .await
        .map_err(|err| format!("socket snapshot: {err}"))?;
    if snap.via != "socket" {
        return Err(format!("snapshot via was {}, not socket", snap.via));
    }
    let session_was_empty = first_pane_id(&snap.raw).is_none();
    let (pane, pane_created_before_marker) = if let Some(id) = first_pane_id(&snap.raw) {
        (id, None)
    } else {
        let created = client
            .rpc(
                "workspace.create",
                json!({ "label": "pendant-bridge-g0", "cwd": "/tmp", "focus": true }),
            )
            .await
            .map_err(|err| format!("workspace.create: {err}"))?;
        if created.get("via").and_then(Value::as_str) != Some("socket") {
            return Err("workspace.create was not via socket".into());
        }
        let id = created
            .pointer("/root_pane/pane_id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .map(str::to_string)
            .ok_or_else(|| {
                format!("workspace.create returned no pane, so the marker check did not run: {created}")
            })?;
        let created_id = id.clone();
        (id, Some(created_id))
    };
    if pane.is_empty() {
        return Err("refusing the marker check with no pane".into());
    }

    let mut sub = client
        .subscribe_events(&[
            json!({ "type": "pane.agent_status_changed", "pane_id": pane }),
            json!({ "type": "pane.updated" }),
        ])
        .await
        .map_err(|err| format!("events.subscribe: {err}"))?;
    if sub.ack.get("via").and_then(Value::as_str) != Some("socket") {
        return Err("subscribe ack was not via socket".into());
    }

    let report_idle = json!({
        "pane_id": pane,
        "source": "g0-scratch",
        "agent": "scratch",
        "state": "idle"
    });
    let report_working = json!({
        "pane_id": pane,
        "source": "g0-scratch",
        "agent": "scratch",
        "state": "working"
    });
    client
        .rpc("pane.report_agent", report_idle)
        .await
        .map_err(|err| format!("report idle: {err}"))?;
    client
        .rpc("pane.report_agent", report_working.clone())
        .await
        .map_err(|err| format!("report working: {err}"))?;

    let status_events = collect_status(&mut sub.events).await?;
    let seq_increases = seq_increases(&status_events);

    let layout_before = client
        .rpc("pane.layout", json!({ "pane_id": pane }))
        .await
        .map_err(|err| format!("layout before: {err}"))?;
    let rect_before = pane_rect(&layout_before, &pane);

    client
        .rpc(
            "pane.send_text",
            json!({ "pane_id": pane, "text": format!("printf '{MARKER}\\n'\n") }),
        )
        .await
        .map_err(|err| format!("send marker: {err}"))?;
    client
        .rpc("pane.send_keys", json!({ "pane_id": pane, "keys": ["enter"] }))
        .await
        .map_err(|err| format!("enter marker: {err}"))?;
    tokio::time::sleep(Duration::from_millis(400)).await;

    let socket_read = client
        .rpc(
            "pane.read",
            json!({
                "pane_id": pane,
                "source": "visible",
                "format": "ansi",
                "strip_ansi": false
            }),
        )
        .await
        .map_err(|err| format!("socket pane.read: {err}"))?;
    let socket_text = socket_read.to_string();
    if socket_read.get("via").and_then(Value::as_str) != Some("socket") {
        return Err("pane.read was not via socket".into());
    }

    let cli_cmd = vec![
        "herdr".to_string(),
        "--session".into(),
        session.clone(),
        "pane".into(),
        "read".into(),
        pane.clone(),
        "--source".into(),
        "visible".into(),
        "--format".into(),
        "ansi".into(),
    ];
    let cli_text = herdr_output(&socket, &session, &cli_cmd[1..]).await?;

    let mut solo = client
        .observe(&pane, 40, 20)
        .await
        .map_err(|err| format!("observe: {err}"))?;
    if solo.fake_binary {
        return Err("scratch check spawned a fake herdr".into());
    }
    let observe_argv = solo.argv.clone();
    let mut frames_a = read_frames(&mut solo, 1, Duration::from_secs(2)).await;
    client
        .rpc(
            "pane.send_text",
            json!({ "pane_id": pane, "text": format!("printf '{INCR}\\n'\n") }),
        )
        .await
        .map_err(|err| format!("send incr: {err}"))?;
    client
        .rpc("pane.send_keys", json!({ "pane_id": pane, "keys": ["enter"] }))
        .await
        .map_err(|err| format!("enter incr: {err}"))?;
    frames_a.extend(read_frames(&mut solo, 4, Duration::from_secs(3)).await);
    let layout_after = client
        .rpc("pane.layout", json!({ "pane_id": pane }))
        .await
        .map_err(|err| format!("layout after: {err}"))?;
    let rect_after = pane_rect(&layout_after, &pane);
    drop(solo);

    let mut first = client
        .observe(&pane, 40, 20)
        .await
        .map_err(|err| format!("observe: {err}"))?;
    let mut second = client
        .observe(&pane, 40, 20)
        .await
        .map_err(|err| format!("second observe: {err}"))?;
    if first.fake_binary || second.fake_binary {
        return Err("scratch check spawned a fake herdr".into());
    }
    let frames_b = read_frames(&mut first, 1, Duration::from_secs(2)).await;
    let frames_c = read_frames(&mut second, 1, Duration::from_secs(2)).await;
    drop(first);
    drop(second);

    let full_false = frames_a
        .iter()
        .find(|frame| frame.get("full").and_then(Value::as_bool) == Some(false));
    let full_false_text = full_false
        .and_then(|frame| frame.get("bytes").and_then(Value::as_str))
        .map(decode_b64_lossy)
        .unwrap_or_default();
    let two = !frames_b.is_empty() && !frames_c.is_empty();
    let resized = match (rect_before, rect_after) {
        (Some(before), Some(after)) => before != after,
        _ => false,
    };
    let observe_cmd = format!(
        "herdr --session {session} terminal session observe {pane} --cols 40 --rows 20"
    );
    let layout_cmd = format!(
        "socket pane.layout {{\"pane_id\":\"{pane}\"}} before and after {observe_cmd}"
    );

    let marker_socket = socket_text.contains(MARKER);
    let marker_cli = cli_text.contains(MARKER);
    let version_ok = version.contains("0.9.5");
    let ok = version_ok
        && marker_socket
        && marker_cli
        && seq_increases
        && !full_false_text.is_empty()
        && !frames_a.is_empty();

    Ok(json!({
        "ok": ok,
        "socket": socket.display().to_string(),
        "session": session.clone(),
        "pane_id": pane.clone(),
        "session_was_empty": session_was_empty,
        "pane_created_before_marker": pane_created_before_marker,
        "herdr_version": version.trim(),
        "g0b": {
            "polling": false,
            "subscribe_connections": 1,
            "status_events": status_events,
            "seq_increases": seq_increases,
            "command": "socket pane.report_agent state idle then working, on the open events.subscribe connection"
        },
        "g0c": {
            "fake_binary": false,
            "herdr_version": version.trim(),
            "observe_argv": observe_argv,
        },
        "g0d": {
            "marker": MARKER,
            "socket_snapshot_has_marker": marker_socket,
            "socket_snapshot_command": "socket pane.read {source:visible, format:ansi}",
            "cli_pane_read_has_marker": marker_cli,
            "cli_pane_read_command": cli_cmd.join(" "),
            "full_false": {
                "contains": full_false_text,
                "frame": full_false.cloned().unwrap_or(Value::Null),
                "solo_flags": frame_flags(&frames_a),
                "command": observe_cmd.clone()
            },
            "two_observers": {
                "answer": if two { "yes" } else { "no" },
                "frames_a": frames_b.len(),
                "frames_b": frames_c.len(),
                "full_flags_a": frame_flags(&frames_b),
                "full_flags_b": frame_flags(&frames_c),
                "command": format!("two concurrent `{observe_cmd}`")
            },
            "phone_resize": {
                "answer": if resized { "yes" } else { "no" },
                "before": rect_before.map(|(w, h)| json!({"width": w, "height": h})).unwrap_or(Value::Null),
                "after": rect_after.map(|(w, h)| json!({"width": w, "height": h})).unwrap_or(Value::Null),
                "command": layout_cmd
            }
        }
    }))
}

fn assert_scratch(path: &Path) -> Result<(), String> {
    let rendered = path.to_string_lossy();
    if named_session(path).is_none() {
        return Err(format!("refusing {rendered}: not a named session socket"));
    }
    if !rendered.contains("/.config/herdr/sessions/") || !rendered.ends_with("/herdr.sock") {
        return Err(format!("refusing {rendered}: scratch socket only"));
    }
    if path == pendant_herdr::default_socket_path() {
        return Err("refusing the live herdr.sock".into());
    }
    if !path_is_socket(path) {
        return Err(format!(
            "scratch socket is not listening at {rendered}. Start it with `herdr --session {} server`",
            named_session(path).unwrap_or_default()
        ));
    }
    Ok(())
}

fn path_is_socket(path: &Path) -> bool {
    use std::os::unix::fs::FileTypeExt;
    std::fs::metadata(path)
        .map(|meta| meta.file_type().is_socket())
        .unwrap_or(false)
}

async fn herdr_version() -> Result<String, String> {
    let output = Command::new("herdr")
        .arg("--version")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|err| format!("herdr --version: {err}"))?;
    if !output.status.success() {
        return Err(format!(
            "herdr --version failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

async fn herdr_output(socket: &Path, session: &str, args: &[String]) -> Result<String, String> {
    let output = Command::new("herdr")
        .args(args)
        .env("HERDR_SOCKET_PATH", socket)
        .env("HERDR_SESSION", session)
        .env("HERDR_SOCKET", socket)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|err| format!("herdr {:?}: {err}", args))?;
    if !output.status.success() {
        return Err(format!(
            "herdr {:?} failed ({}): {}",
            args,
            output.status,
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

async fn collect_status(events: &mut pendant_herdr::EventStream) -> Result<Vec<Value>, String> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    let mut matched = Vec::new();
    while tokio::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let next = timeout(remaining, events.next_event()).await;
        let event = match next {
            Ok(Ok(event)) => event,
            Ok(Err(err)) => return Err(format!("subscribe read: {err}")),
            Err(_) => break,
        };
        if event.get("via").and_then(Value::as_str) != Some("socket") {
            return Err("event was not via socket".into());
        }
        if event.get("seq").and_then(Value::as_u64).is_none() {
            return Err("subscribe event has no seq".into());
        }
        let name = event
            .get("event")
            .and_then(Value::as_str)
            .unwrap_or("");
        if name.contains("agent_status") {
            let status = event
                .pointer("/data/agent_status")
                .and_then(Value::as_str)
                .unwrap_or("");
            if status == "idle" || status == "working" {
                matched.push(event);
            }
        }
        let saw_idle = matched.iter().any(|event| status_of(event) == "idle");
        let saw_working = matched.iter().any(|event| status_of(event) == "working");
        if saw_idle && saw_working {
            break;
        }
    }
    if !seq_increases(&matched) {
        return Err(format!(
            "two live status changes did not show an increasing seq: {matched:?}"
        ));
    }
    Ok(matched)
}

fn status_of(event: &Value) -> &str {
    event
        .pointer("/data/agent_status")
        .and_then(Value::as_str)
        .unwrap_or("")
}

fn seq_increases(events: &[Value]) -> bool {
    let idle = events.iter().find(|event| status_of(event) == "idle");
    let working = events.iter().find(|event| status_of(event) == "working");
    match (idle, working) {
        (Some(idle), Some(working)) => {
            let idle_seq = idle.get("seq").and_then(Value::as_u64);
            let working_seq = working.get("seq").and_then(Value::as_u64);
            matches!((idle_seq, working_seq), (Some(earlier), Some(later)) if earlier < later)
        }
        _ => false,
    }
}

async fn read_frames(
    session: &mut pendant_herdr::ObserveSession,
    limit: usize,
    wait: Duration,
) -> Vec<Value> {
    let mut frames = Vec::new();
    for _ in 0..limit {
        match timeout(wait, session.next_frame()).await {
            Ok(Ok(frame)) => frames.push(frame.raw),
            _ => break,
        }
    }
    frames
}

fn frame_flags(frames: &[Value]) -> Vec<Value> {
    frames
        .iter()
        .map(|frame| {
            json!({
                "seq": frame.get("seq").cloned().unwrap_or(Value::Null),
                "full": frame.get("full").cloned().unwrap_or(Value::Null)
            })
        })
        .collect()
}

fn first_pane_id(raw: &Value) -> Option<String> {
    for pointer in [
        "/focused_pane_id",
        "/snapshot/focused_pane_id",
        "/result/focused_pane_id",
        "/result/snapshot/focused_pane_id",
    ] {
        if let Some(id) = raw.pointer(pointer).and_then(Value::as_str) {
            if !id.is_empty() {
                return Some(id.to_string());
            }
        }
    }
    let mut found = None;
    walk_pane(raw, &mut found);
    found
}

fn walk_pane(value: &Value, found: &mut Option<String>) {
    if found.is_some() {
        return;
    }
    match value {
        Value::Object(map) => {
            if let Some(id) = map.get("pane_id").and_then(Value::as_str) {
                if !id.is_empty() {
                    *found = Some(id.to_string());
                    return;
                }
            }
            for child in map.values() {
                walk_pane(child, found);
            }
        }
        Value::Array(items) => {
            for child in items {
                walk_pane(child, found);
            }
        }
        _ => {}
    }
}

fn pane_rect(layout: &Value, pane_id: &str) -> Option<(u64, u64)> {
    fn walk(value: &Value, pane_id: &str) -> Option<(u64, u64)> {
        match value {
            Value::Object(map) => {
                if map.get("pane_id").and_then(Value::as_str) == Some(pane_id) {
                    if let Some(rect) = map.get("rect").or_else(|| map.get("geometry")) {
                        let width = rect.get("width").and_then(Value::as_u64);
                        let height = rect.get("height").and_then(Value::as_u64);
                        if let (Some(width), Some(height)) = (width, height) {
                            return Some((width, height));
                        }
                    }
                }
                for child in map.values() {
                    if let Some(found) = walk(child, pane_id) {
                        return Some(found);
                    }
                }
                None
            }
            Value::Array(items) => items.iter().find_map(|child| walk(child, pane_id)),
            _ => None,
        }
    }
    walk(layout, pane_id)
}

fn decode_b64_lossy(input: &str) -> String {
    let bytes = b64_decode(input).unwrap_or_default();
    let text = String::from_utf8_lossy(&bytes);
    let mut trimmed = text.chars().take(400).collect::<String>();
    if text.chars().count() > 400 {
        trimmed.push('…');
    }
    trimmed
}

fn b64_decode(input: &str) -> Result<Vec<u8>, ()> {
    fn val(c: u8) -> Result<u8, ()> {
        match c {
            b'A'..=b'Z' => Ok(c - b'A'),
            b'a'..=b'z' => Ok(c - b'a' + 26),
            b'0'..=b'9' => Ok(c - b'0' + 52),
            b'+' => Ok(62),
            b'/' => Ok(63),
            _ => Err(()),
        }
    }
    let body: Vec<u8> = input
        .bytes()
        .filter(|byte| !byte.is_ascii_whitespace() && *byte != b'=')
        .collect();
    let mut out = Vec::new();
    let mut buf: u32 = 0;
    let mut bits: u32 = 0;
    for byte in body {
        let value = val(byte)?;
        buf = (buf << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }
    Ok(out)
}
