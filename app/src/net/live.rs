//! The live link to the bridge, shared by every screen.
//!
//! - One worker thread holds the WebSocket (`wss://<bridge>/v1/ws`) with
//!   blocking tungstenite over rustls, and hands what arrives to the engine
//!   thread through a channel. nsh 0.5.0's own WebSocket is reachable from
//!   TSX only; this path needs no async runtime.
//! - The engine thread drains that channel on a short interval into
//!   [`Live`]'s signals, and polls `/v1/snapshot` and `/v1/events` with
//!   nsh's `fetch` every [`POLL`].
//!
//! Screens read [`live()`]: `link`, `snapshot`, `inbox`, `error` and
//! `panes`, and call `watch` / `unwatch` for the panes they show.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::io::ErrorKind;
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use nsh_core::prelude::*;
use tungstenite::client::IntoClientRequest;
use tungstenite::http::HeaderValue;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Connector, Message, WebSocket};

use super::ws::{Down, Frame, SeqTrack, Up};
use super::{Bridge, InboxItem, Snapshot};

/// How often the snapshot and the inbox are fetched again.
pub const POLL: Duration = Duration::from_secs(3);
/// How often the engine thread takes what the worker received.
const DRAIN: Duration = Duration::from_millis(100);
/// How long one socket read waits before the worker looks at its commands.
const READ_TICK: Duration = Duration::from_millis(200);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const BACKOFF_MAX: Duration = Duration::from_secs(30);
/// The terminal size asked for when a screen watches a pane.
const COLS: u16 = 80;
const ROWS: u16 = 24;

/// The WebSocket's state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Link {
    /// This build has no usable bridge address.
    Unconfigured(String),
    Connecting,
    Open,
    /// Lost or refused; the worker tries again with a growing pause.
    Closed(String),
}

/// What one watched pane has received.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PaneFeed {
    /// Frames seen, the highest `seq`, and any that did not rise.
    pub seq: SeqTrack,
    /// How many of the frames were full ones.
    pub full_frames: u64,
    /// The last frame (base64 ANSI in `bytes`), for the terminal pack.
    pub last: Option<Frame>,
    /// The pane's recent text, sent once when it is watched.
    pub scrollback: Option<String>,
    /// The last operation the bridge refused for this pane.
    pub refused: Option<String>,
}

/// The shared live state. Copy it freely; it is a handle to signals.
#[derive(Clone, Copy)]
pub struct Live {
    pub link: Signal<Link>,
    /// The last snapshot fetched; `None` until the first one arrives.
    pub snapshot: Signal<Option<Snapshot>>,
    /// The inbox (`/v1/events`): agents that are blocked or done.
    pub inbox: Signal<Vec<InboxItem>>,
    /// The last HTTP failure, cleared by the next success.
    pub error: Signal<Option<String>>,
    /// Watched panes by pane id.
    pub panes: Signal<BTreeMap<String, PaneFeed>>,
}

enum Cmd {
    Watch { pane: String, cols: u16, rows: u16 },
    Unwatch(String),
}

enum Event {
    Link(Link),
    Down(Down),
}

thread_local! {
    /// The worker's command channel, owned by the engine thread.
    static COMMANDS: RefCell<Option<Sender<Cmd>>> = const { RefCell::new(None) };
}

impl Live {
    /// Start frames for `pane` (the agent's `pane_id`).
    pub fn watch(&self, pane: &str) {
        command(Cmd::Watch {
            pane: pane.to_owned(),
            cols: COLS,
            rows: ROWS,
        });
    }

    /// Stop frames for `pane` and forget what it received.
    pub fn unwatch(&self, pane: &str) {
        command(Cmd::Unwatch(pane.to_owned()));
        self.panes.update(|panes| {
            panes.remove(pane);
        });
    }

    /// Blocked items in the inbox.
    pub fn blocked(&self) -> usize {
        self.inbox
            .with(|items| items.iter().filter(|item| item.is_blocked()).count())
    }
}

fn command(cmd: Cmd) {
    COMMANDS.with(|commands| {
        if let Some(tx) = commands.borrow().as_ref() {
            let _ = tx.send(cmd);
        }
    });
}

/// The live state the root provided.
///
/// # Panics
/// Outside the app's root, where [`start`] has not run.
pub fn live() -> Live {
    use_context::<Live>().expect("net::live::start runs in the app's root")
}

/// Starts the link and provides [`Live`] to everything the current owner
/// makes. The root calls it once; the worker and the polling stop with it.
pub fn start() -> Live {
    let live = Live {
        link: signal(Link::Connecting),
        snapshot: signal(None),
        inbox: signal(Vec::new()),
        error: signal(None),
        panes: signal(BTreeMap::new()),
    };
    provide_context(live);
    let bridge = match crate::bridge() {
        Ok(bridge) => bridge,
        Err(err) => {
            live.link.set(Link::Unconfigured(err.to_string()));
            return live;
        }
    };

    let (cmd_tx, cmd_rx) = mpsc::channel();
    let (event_tx, event_rx) = mpsc::channel();
    let worker_bridge = bridge.clone();
    let spawned = std::thread::Builder::new()
        .name("pendant-ws".into())
        .spawn(move || worker(worker_bridge, cmd_rx, event_tx));
    if let Err(err) = spawned {
        live.link
            .set(Link::Closed(format!("no worker thread: {err}")));
    }
    COMMANDS.with(|commands| *commands.borrow_mut() = Some(cmd_tx));

    let drain = set_interval(DRAIN, move || drain(&event_rx, live));
    let poll = spawn(async move {
        loop {
            refresh(&bridge, live).await;
            timer(POLL).await;
        }
    });
    // Dropping a task cancels it, so the owner keeps both until it goes;
    // dropping the sender then ends the worker.
    cleanup(move || {
        drop(drain);
        drop(poll);
        COMMANDS.with(|commands| commands.borrow_mut().take());
    });
    live
}

async fn refresh(bridge: &Bridge, live: Live) {
    let snapshot = bridge.snapshot().await;
    let inbox = bridge.inbox().await;
    let mut failure = None;
    match snapshot {
        Ok(snapshot) => live.snapshot.set(Some(snapshot)),
        Err(err) => failure = Some(err.to_string()),
    }
    match inbox {
        Ok(inbox) => live.inbox.set(inbox.items),
        Err(err) => failure = failure.or(Some(err.to_string())),
    }
    live.error.set(failure);
}

fn drain(events: &Receiver<Event>, live: Live) {
    let mut downs = Vec::new();
    while let Ok(event) = events.try_recv() {
        match event {
            Event::Link(link) => live.link.set(link),
            Event::Down(down) => downs.push(down),
        }
    }
    if !downs.is_empty() {
        live.panes.update(|panes| {
            for down in downs {
                apply(panes, down);
            }
        });
    }
}

/// Folds one message into the panes' state.
pub fn apply(panes: &mut BTreeMap<String, PaneFeed>, down: Down) {
    match down {
        Down::Frame(frame) => {
            let feed = panes.entry(frame.pane.clone()).or_default();
            feed.seq.observe(frame.seq);
            if frame.full {
                feed.full_frames += 1;
            }
            trace(&format!(
                "frame pane={} seq={} full={} frames={}",
                frame.pane, frame.seq, frame.full, feed.seq.frames
            ));
            feed.last = Some(frame);
        }
        Down::Scrollback { pane, text, .. } => {
            panes.entry(pane).or_default().scrollback = Some(text);
        }
        Down::Refused { pane, op, .. } => {
            panes.entry(pane).or_default().refused = Some(op);
        }
        Down::Unknown => {}
    }
}

/// One line to the platform log (logcat on Android) and, on iOS, to the
/// app's stderr, which `simctl launch --console` shows.
fn trace(line: &str) {
    log::info!(target: "pendant", "{line}");
    #[cfg(target_os = "ios")]
    eprintln!("pendant: {line}");
}

// The worker thread.

enum End {
    /// The engine side went away.
    Stop,
    /// The socket ended or failed; try again.
    Lost(String),
}

fn worker(bridge: Bridge, cmds: Receiver<Cmd>, events: Sender<Event>) {
    let tls = tls_config();
    let mut watched: BTreeMap<String, (u16, u16)> = BTreeMap::new();
    let mut backoff = Duration::from_secs(1);
    loop {
        if events.send(Event::Link(Link::Connecting)).is_err() || !absorb(&cmds, &mut watched) {
            return;
        }
        let lost = match connect(&bridge, tls.clone()) {
            Ok(mut socket) => {
                backoff = Duration::from_secs(1);
                trace(&format!("open {}", bridge.ws_url()));
                if events.send(Event::Link(Link::Open)).is_err() {
                    return;
                }
                match session(&mut socket, &cmds, &events, &mut watched) {
                    End::Stop => {
                        let _ = socket.close(None);
                        let _ = socket.flush();
                        return;
                    }
                    End::Lost(why) => why,
                }
            }
            Err(why) => why,
        };
        trace(&format!("closed: {lost}"));
        if events.send(Event::Link(Link::Closed(lost))).is_err() {
            return;
        }
        let until = Instant::now() + backoff;
        while Instant::now() < until {
            std::thread::sleep(READ_TICK);
            if !absorb(&cmds, &mut watched) {
                return;
            }
        }
        backoff = (backoff * 2).min(BACKOFF_MAX);
    }
}

/// Takes queued commands while the socket is down. `false` once the engine
/// side has gone.
fn absorb(cmds: &Receiver<Cmd>, watched: &mut BTreeMap<String, (u16, u16)>) -> bool {
    loop {
        match cmds.try_recv() {
            Ok(Cmd::Watch { pane, cols, rows }) => {
                watched.insert(pane, (cols, rows));
            }
            Ok(Cmd::Unwatch(pane)) => {
                watched.remove(&pane);
            }
            Err(TryRecvError::Empty) => return true,
            Err(TryRecvError::Disconnected) => return false,
        }
    }
}

type Socket = WebSocket<MaybeTlsStream<TcpStream>>;

fn session(
    socket: &mut Socket,
    cmds: &Receiver<Cmd>,
    events: &Sender<Event>,
    watched: &mut BTreeMap<String, (u16, u16)>,
) -> End {
    // Watches made before this socket opened (or before a reconnect).
    let panes: BTreeSet<_> = watched.keys().cloned().collect();
    for pane in panes {
        let (cols, rows) = watched[&pane];
        if let Err(why) = send(socket, &Up::watch(pane, cols, rows)) {
            return End::Lost(why);
        }
    }
    loop {
        loop {
            let up = match cmds.try_recv() {
                Ok(Cmd::Watch { pane, cols, rows }) => {
                    watched.insert(pane.clone(), (cols, rows));
                    Up::watch(pane, cols, rows)
                }
                Ok(Cmd::Unwatch(pane)) => {
                    watched.remove(&pane);
                    Up::unwatch(pane)
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return End::Stop,
            };
            if let Err(why) = send(socket, &up) {
                return End::Lost(why);
            }
        }
        match socket.read() {
            Ok(Message::Text(text)) => {
                if let Some(down) = Down::parse(text.as_str()) {
                    if events.send(Event::Down(down)).is_err() {
                        return End::Stop;
                    }
                }
            }
            Ok(Message::Close(_)) => return End::Lost("the bridge closed the socket".into()),
            Ok(_) => {}
            Err(tungstenite::Error::Io(err)) if is_tick(&err) => {
                // Read timeout: a chance to send pongs and take commands.
                match socket.flush() {
                    Ok(()) => {}
                    Err(tungstenite::Error::Io(err)) if is_tick(&err) => {}
                    Err(err) => return End::Lost(err.to_string()),
                }
            }
            Err(err) => return End::Lost(err.to_string()),
        }
    }
}

fn is_tick(err: &std::io::Error) -> bool {
    matches!(err.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut)
}

fn send(socket: &mut Socket, up: &Up) -> Result<(), String> {
    socket
        .send(Message::text(up.to_json()))
        .map_err(|err| err.to_string())
}

fn connect(bridge: &Bridge, tls: Arc<rustls::ClientConfig>) -> Result<Socket, String> {
    let (host, port) = bridge.host_port();
    let addrs = (host, port)
        .to_socket_addrs()
        .map_err(|err| format!("cannot resolve the bridge: {err}"))?;
    let mut last = String::from("no address for the bridge");
    let mut tcp = None;
    for addr in addrs {
        match TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT) {
            Ok(stream) => {
                tcp = Some(stream);
                break;
            }
            Err(err) => last = format!("cannot connect: {err}"),
        }
    }
    let tcp = tcp.ok_or(last)?;
    let setup = |tcp: &TcpStream| -> std::io::Result<TcpStream> {
        tcp.set_nodelay(true)?;
        tcp.set_read_timeout(Some(CONNECT_TIMEOUT))?;
        tcp.set_write_timeout(Some(CONNECT_TIMEOUT))?;
        tcp.try_clone()
    };
    let handle = setup(&tcp).map_err(|err| format!("socket setup: {err}"))?;

    let mut request = bridge
        .ws_url()
        .into_client_request()
        .map_err(|err| format!("bad request: {err}"))?;
    // The bridge accepts an upgrade only from its own Serve origin.
    let origin = HeaderValue::from_str(bridge.origin()).map_err(|err| err.to_string())?;
    request.headers_mut().insert("Origin", origin);

    let (socket, _response) =
        tungstenite::client_tls_with_config(request, tcp, None, Some(Connector::Rustls(tls)))
            .map_err(|err| format!("handshake: {err}"))?;
    // From here a read waits at most one tick, so commands are not stuck
    // behind a quiet socket.
    handle
        .set_read_timeout(Some(READ_TICK))
        .map_err(|err| format!("socket setup: {err}"))?;
    Ok(socket)
}

/// rustls with ring and the webpki roots: no platform TLS, no async runtime.
fn tls_config() -> Arc<rustls::ClientConfig> {
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .expect("ring supports the default TLS versions")
    .with_root_certificates(roots)
    .with_no_client_auth();
    Arc::new(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(pane: &str, seq: u64, full: bool) -> Down {
        Down::Frame(Frame {
            v: 1,
            pane: pane.into(),
            full,
            seq,
            bytes: String::new(),
            width: 80,
            height: 24,
        })
    }

    #[test]
    fn frames_fold_into_their_pane() {
        let mut panes = BTreeMap::new();
        for down in [
            Down::Scrollback {
                v: 1,
                pane: "w1:p2".into(),
                text: "MARKER-B".into(),
            },
            frame("w1:p2", 1, true),
            frame("w1:p2", 2, false),
            frame("w1:p3", 5, true),
            frame("w1:p2", 2, false),
            Down::Refused {
                v: 1,
                pane: "w1:p2".into(),
                op: "input".into(),
            },
            Down::Unknown,
        ] {
            apply(&mut panes, down);
        }
        let p2 = &panes["w1:p2"];
        assert_eq!(p2.seq.frames, 3);
        assert_eq!(p2.seq.last(), Some(2));
        assert_eq!(p2.seq.out_of_order, 1);
        assert_eq!(p2.full_frames, 1);
        assert_eq!(p2.scrollback.as_deref(), Some("MARKER-B"));
        assert_eq!(p2.refused.as_deref(), Some("input"));
        assert_eq!(panes["w1:p3"].seq.last(), Some(5));
        assert_eq!(panes.len(), 2);
    }

    #[test]
    fn queued_commands_survive_until_the_socket_opens() {
        let (tx, rx) = mpsc::channel();
        let mut watched = BTreeMap::new();
        tx.send(Cmd::Watch {
            pane: "w1:p1".into(),
            cols: 80,
            rows: 24,
        })
        .unwrap();
        tx.send(Cmd::Watch {
            pane: "w1:p2".into(),
            cols: 100,
            rows: 30,
        })
        .unwrap();
        tx.send(Cmd::Unwatch("w1:p1".into())).unwrap();
        assert!(absorb(&rx, &mut watched));
        assert_eq!(
            watched.into_iter().collect::<Vec<_>>(),
            [("w1:p2".to_owned(), (100, 30))]
        );
        drop(tx);
        assert!(!absorb(&rx, &mut BTreeMap::new()));
    }

    /// Drives the worker against a running bridge:
    /// `PENDANT_BRIDGE_URL=https://… PENDANT_WATCH_PANE=w1:p1 cargo test
    /// --manifest-path app/Cargo.toml -- --ignored --nocapture live_socket`.
    #[test]
    #[ignore = "needs a running bridge (PENDANT_BRIDGE_URL at run time)"]
    fn live_socket_sees_rising_frames() {
        let url = std::env::var("PENDANT_BRIDGE_URL").expect("PENDANT_BRIDGE_URL");
        let pane = std::env::var("PENDANT_WATCH_PANE").unwrap_or_else(|_| "w1:p1".into());
        let want: u64 = std::env::var("PENDANT_WANT_FRAMES")
            .ok()
            .and_then(|n| n.parse().ok())
            .unwrap_or(3);
        let bridge = Bridge::parse(&url).expect("an https origin");
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        let worker = std::thread::spawn(move || worker(bridge, cmd_rx, event_tx));
        cmd_tx
            .send(Cmd::Watch {
                pane: pane.clone(),
                cols: COLS,
                rows: ROWS,
            })
            .unwrap();

        let mut panes = BTreeMap::new();
        let mut links = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            match event_rx.recv_timeout(Duration::from_millis(500)) {
                Ok(Event::Link(link)) => links.push(link),
                Ok(Event::Down(down)) => apply(&mut panes, down),
                Err(_) => {}
            }
            if panes.get(&pane).is_some_and(|feed| feed.seq.frames >= want) {
                break;
            }
        }
        drop(cmd_tx);
        worker.join().unwrap();

        let feed = panes.get(&pane).cloned().unwrap_or_default();
        println!(
            "links={links:?} pane={pane} frames={} last_seq={:?} out_of_order={} full={} scrollback_chars={}",
            feed.seq.frames,
            feed.seq.last(),
            feed.seq.out_of_order,
            feed.full_frames,
            feed.scrollback.as_deref().map_or(0, str::len),
        );
        assert!(
            links.contains(&Link::Open),
            "the socket never opened: {links:?}"
        );
        assert!(feed.seq.frames >= want, "too few frames");
        assert_eq!(feed.seq.out_of_order, 0, "seq did not rise");
    }

    #[test]
    fn the_tls_config_builds_with_roots() {
        let config = tls_config();
        assert!(!config.alpn_protocols.iter().any(|p| p == b"h2"));
    }
}
