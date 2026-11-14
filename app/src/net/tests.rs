//! The wire types against JSON shaped like the bridge's replies. All names
//! are made up; the host is the `example.ts.net` placeholder.

use super::ws::{Down, Frame, SeqTrack, Up};
use super::*;

const SNAPSHOT: &str = r#"{
  "agents": [
    {"id":"w1:p1","name":null,"agent":"claude","status":"working","pane_id":"w1:p1","workspace_id":"w1","tab_id":"w1:t1","title":"alpha"},
    {"id":"w1:p2","name":null,"agent":"claude","status":"blocked","pane_id":"w1:p2","workspace_id":"w1","tab_id":"w1:t1","title":"bravo"},
    {"id":"w1:p3","name":null,"agent":"codex","status":"idle","pane_id":"w1:p3","workspace_id":"w1","tab_id":"w1:t2","title":null},
    {"id":"scout","name":"scout","agent":null,"status":"done","pane_id":"w1:p4","workspace_id":"w1","tab_id":null,"title":""}
  ],
  "workspaces": [{"workspace_id":"w1","label":"scratch","cwd":"/tmp/scratch","worktree":null}],
  "raw": {"via":"socket","result":{"agents":[]}}
}"#;

#[test]
fn a_snapshot_reads_and_ignores_raw() {
    let snap: Snapshot = decode(200, SNAPSHOT).unwrap();
    assert_eq!(snap.agents.len(), 4);
    assert_eq!(snap.workspaces[0].label.as_deref(), Some("scratch"));
    let blocked: Vec<_> = snap.blocked().map(|agent| agent.id.as_str()).collect();
    assert_eq!(blocked, ["w1:p2"]);
    assert_eq!(snap.agent("w1:p4").map(|a| a.id.as_str()), Some("scout"));
    assert_eq!(
        snap.agent("scout").map(|a| a.pane_id.as_str()),
        Some("w1:p4")
    );
}

#[test]
fn a_label_falls_back_from_title_to_name_to_id() {
    let snap: Snapshot = decode(200, SNAPSHOT).unwrap();
    let labels: Vec<_> = snap.agents.iter().map(Agent::label).collect();
    assert_eq!(labels, ["alpha", "bravo", "w1:p3", "scout"]);
}

#[test]
fn an_agent_detail_finds_the_output_text() {
    let body = r#"{"id":"w1:p2","agent":{"id":"w1:p2","name":null,"agent":"claude","status":"blocked","pane_id":"w1:p2","workspace_id":"w1","tab_id":null,"title":"bravo"},
        "get":{"status":"blocked"},"output":{"read":{"text":"MARKER-B line\n"}},"blocked":true}"#;
    let detail: AgentDetail = decode(200, body).unwrap();
    assert!(detail.blocked);
    assert!(detail.agent.as_ref().unwrap().is_blocked());
    assert_eq!(detail.output_text(), Some("MARKER-B line\n"));

    let gone = r#"{"id":"w9:p9","agent":null,"get":null,"output":null,"blocked":false}"#;
    let detail: AgentDetail = decode(200, gone).unwrap();
    assert_eq!(detail.agent, None);
    assert_eq!(detail.output_text(), None);
}

#[test]
fn the_inbox_reads_its_items() {
    let body = r#"{"items":[{"id":"w1:p2","agent":"claude","status":"blocked","workspace_id":"w1","title":"bravo","kind":"blocked"},
        {"id":"scout","agent":"scout","status":"done","workspace_id":"w1","title":null,"kind":"done"}]}"#;
    let inbox: Inbox = decode(200, body).unwrap();
    assert_eq!(inbox.items.len(), 2);
    assert!(inbox.items[0].is_blocked());
    assert!(!inbox.items[1].is_blocked());
}

#[test]
fn an_error_status_keeps_the_bridge_code() {
    let body = r#"{"error":{"code":"host_refused","message":"host is not allowed"}}"#;
    assert_eq!(
        decode::<Snapshot>(403, body),
        Err(NetError::Status {
            status: 403,
            code: Some("host_refused".into())
        })
    );
    assert_eq!(
        decode::<Snapshot>(502, "bad gateway"),
        Err(NetError::Status {
            status: 502,
            code: None
        })
    );
    assert!(matches!(
        decode::<Snapshot>(200, "{}"),
        Err(NetError::Decode(_))
    ));
}

#[test]
fn the_bridge_address_is_an_https_origin() {
    let bridge = Bridge::parse("https://Bridge.Example.ts.net:8443/").unwrap();
    assert_eq!(bridge.origin(), "https://bridge.example.ts.net:8443");
    assert_eq!(bridge.ws_url(), "wss://bridge.example.ts.net:8443/v1/ws");
    assert_eq!(
        bridge.url("/v1/snapshot"),
        "https://bridge.example.ts.net:8443/v1/snapshot"
    );
    assert!(Bridge::parse("https://bridge.example.ts.net").is_ok());

    for bad in [
        "http://bridge.example.ts.net:8443",
        "https://",
        "https://bridge.example.ts.net:8443/v1",
        "https://user@bridge.example.ts.net",
        "https://bridge.example.ts.net:0",
        "https://bridge.example.ts.net:http",
        "https://[::1]:8443",
        "bridge.example.ts.net",
    ] {
        assert!(
            matches!(Bridge::parse(bad), Err(ConfigError::Invalid(_))),
            "{bad}"
        );
    }
}

#[test]
fn agent_ids_are_one_path_segment() {
    assert_eq!(encode_segment("w1:p2"), "w1:p2");
    assert_eq!(encode_segment("a/b?c d"), "a%2Fb%3Fc%20d");
}

#[test]
fn watch_and_frames_match_the_envelope() {
    assert_eq!(
        Up::watch("w1:p2", 80, 24).to_json(),
        r#"{"v":1,"pane":"w1:p2","type":"watch","cols":80,"rows":24}"#
    );
    assert_eq!(
        Up::unwatch("w1:p2").to_json(),
        r#"{"v":1,"pane":"w1:p2","type":"unwatch"}"#
    );
    assert_eq!(
        Up::resync("w1:p2").to_json(),
        r#"{"v":1,"pane":"w1:p2","type":"resync"}"#
    );

    let frame = r#"{"v":1,"pane":"w1:p2","type":"frame","full":true,"seq":7,"bytes":"G1sySg==","width":80,"height":24}"#;
    assert_eq!(
        Down::parse(frame),
        Some(Down::Frame(Frame {
            v: 1,
            pane: "w1:p2".into(),
            full: true,
            seq: 7,
            bytes: "G1sySg==".into(),
            width: 80,
            height: 24,
        }))
    );
    let scroll = r#"{"v":1,"pane":"w1:p2","type":"scrollback","text":"MARKER-B"}"#;
    assert!(
        matches!(Down::parse(scroll), Some(Down::Scrollback { text, .. }) if text == "MARKER-B")
    );
    let refused = r#"{"v":1,"pane":"w1:p2","type":"refused","op":"input"}"#;
    assert!(matches!(Down::parse(refused), Some(Down::Refused { op, .. }) if op == "input"));
    assert_eq!(
        Down::parse(r#"{"v":1,"pane":"x","type":"later"}"#),
        Some(Down::Unknown)
    );
    assert_eq!(Down::parse(&frame.replace(r#""v":1"#, r#""v":2"#)), None);
    assert_eq!(Down::parse("not json"), None);
}

#[test]
fn seq_tracking_counts_frames_and_spots_a_step_back() {
    let mut track = SeqTrack::default();
    for seq in [3, 4, 9, 9, 10] {
        track.observe(seq);
    }
    assert_eq!(track.frames, 5);
    assert_eq!(track.out_of_order, 1);
    assert_eq!(track.last(), Some(10));
}
