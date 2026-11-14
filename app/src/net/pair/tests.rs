//! The pairing wire format against the bridge's (`pairing.rs`, `routes.rs`).
//! The nonces and ids here are made up.

use super::*;

const NONCE: &str = "00112233445566778899aabbccddeeff";

#[test]
fn a_payload_is_the_bridge_prefix_and_a_hex_nonce() {
    let payload = Payload::parse(&format!("  pendant-pair:v1:{NONCE}\n")).unwrap();
    assert_eq!(payload.nonce, NONCE);
    for bad in [
        String::new(),
        NONCE.to_owned(),
        format!("pendant-pair:v2:{NONCE}"),
        format!("pendant-pair:v1:{}", &NONCE[..31]),
        format!("pendant-pair:v1:{NONCE}0"),
        format!("pendant-pair:v1:{}", NONCE.to_uppercase()),
        "pendant-pair:v1:zz112233445566778899aabbccddeeff".to_owned(),
    ] {
        assert_eq!(Payload::parse(&bad), Err(WriteError::BadPayload), "{bad:?}");
    }
}

#[test]
fn a_payload_never_prints_its_nonce() {
    let payload = Payload::parse(&format!("pendant-pair:v1:{NONCE}")).unwrap();
    let shown = format!("{payload:?}");
    assert!(!shown.contains(NONCE), "{shown}");
    assert_eq!(shown, "Payload(<redacted>)");
}

#[test]
fn the_signed_messages_are_the_bridges() {
    assert_eq!(
        pair_message("pendant-01", "BAAA", NONCE),
        format!("pendant-pair:v1\npendant-01\nBAAA\n{NONCE}").into_bytes()
    );
    assert_eq!(
        grant_message("pendant-01", "c1"),
        b"pendant-grant:v1\npendant-01\nc1".to_vec()
    );
    assert_eq!(
        sign_message("pendant-01", "POST", "/v1/agents/w1:p2/approve", "c2", "y"),
        b"pendant-sign:v1\npendant-01\nPOST\n/v1/agents/w1:p2/approve\nc2\ny".to_vec()
    );
}

#[test]
fn write_paths_match_the_bridge_routes() {
    assert_eq!(agent_path("w1:p2", "prompt"), "/v1/agents/w1:p2/prompt");
    assert_eq!(agent_path("w1:p2", "approve"), "/v1/agents/w1:p2/approve");
    assert_eq!(Decision::Approve.key(), "y");
    assert_eq!(Decision::Deny.key(), "n");
}

#[test]
fn a_worktree_body_is_the_bridges_reserialization() {
    // The bridge signs `serde_json::to_string(&CreateWorktree)`: every field,
    // in its order, nulls included.
    let request = WorktreeRequest {
        workspace_id: Some("w1".into()),
        branch: Some("scratch/g2".into()),
        label: Some("g2".into()),
        ..Default::default()
    };
    assert_eq!(
        request.canonical(),
        r#"{"workspace_id":"w1","cwd":null,"branch":"scratch/g2","base":null,"path":null,"label":"g2","focus":false}"#
    );
}

#[test]
fn the_device_id_follows_the_public_key() {
    let mut key = vec![0x04];
    key.extend(0x10..0x50u8);
    assert_eq!(device_id(&key), "pendant-1011121314151617");
    key[1] = 0xff;
    assert_eq!(device_id(&key), "pendant-ff11121314151617");
}

#[test]
fn bridge_refusals_map_to_what_the_app_does_next() {
    let status = |code: &str| NetError::Status {
        status: 403,
        code: Some(code.into()),
    };
    assert_eq!(WriteError::from(status("unpaired")), WriteError::Unpaired);
    assert_eq!(WriteError::from(status("revoked")), WriteError::Unpaired);
    assert_eq!(
        WriteError::from(status("grant_required")),
        WriteError::GrantRequired
    );
    assert_eq!(
        WriteError::from(status("grant_released")),
        WriteError::GrantReleased
    );
    assert_eq!(
        WriteError::from(status("takeover_off")),
        WriteError::TakeoverOff
    );
    assert_eq!(
        WriteError::from(status("key_refused")),
        WriteError::Refused {
            status: 403,
            code: Some("key_refused".into())
        }
    );
    assert_eq!(
        WriteError::from(NetError::Transport("down".into())),
        WriteError::Net(NetError::Transport("down".into()))
    );
}

#[test]
fn work_off_the_engine_thread_resolves() {
    let main = std::thread::current().id();
    let future = off_thread(move || std::thread::current().id() != main);
    assert_eq!(block_on(future), Some(true));
}

/// A minimal executor for the test: polls on a thread-parking waker.
fn block_on<F: Future>(future: F) -> F::Output {
    use std::task::Wake;
    struct Unpark(std::thread::Thread);
    impl Wake for Unpark {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
    }
    let waker = Waker::from(Arc::new(Unpark(std::thread::current())));
    let mut cx = Context::from_waker(&waker);
    let mut future = std::pin::pin!(future);
    loop {
        if let Poll::Ready(value) = future.as_mut().poll(&mut cx) {
            return value;
        }
        std::thread::park();
    }
}
