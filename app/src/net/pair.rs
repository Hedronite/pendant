//! Pairing and signed writes, as the bridge's `pairing.rs` and its write
//! routes verify them.
//!
//! - **Pair:** the payload the bridge host shows (`pendant-pair:v1:<nonce>`)
//!   is a one-time secret. The device key signs `pendant-pair:v1` over the
//!   device id, its public key and the nonce, and `POST /v1/pair` stores the
//!   key. Takeover is off: while one device is paired, another is refused.
//! - **Grant:** a fresh `GET /v1/challenge`, signed as `pendant-grant:v1`,
//!   opens a grant (`POST /v1/grant`) that the bridge keeps for 30 s.
//! - **Write:** each write takes its own challenge and signs
//!   `pendant-sign:v1\n{device}\n{method}\n{path}\n{challenge}\n{canonical}`,
//!   sent in the `x-pendant-device`, `x-pendant-signature` and
//!   `x-pendant-challenge` headers.
//!
//! Every signature goes through the platform device key ([`crate::keystore`]),
//! which asks for a biometric each time, so it runs off the engine thread.
//! The nonce is never logged, stored or shown.

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use nsh_core::prelude::*;
use serde::Serialize;

use super::{Bridge, NetError};
use crate::keystore::{DeviceKey, KeyError};

/// The prefix of the payload the bridge issues.
pub const PAYLOAD_PREFIX: &str = "pendant-pair:v1:";

/// How long the app treats a grant as open. The bridge keeps it 30 s; the
/// app stops a little early so a write does not race the expiry.
pub const GRANT_WINDOW: std::time::Duration = std::time::Duration::from_secs(25);

/// The file, in the app's data directory, that keeps the paired device id
/// between launches. Not a secret: the key stays in the Keychain or
/// Keystore. (nsh's `Storage` is not used: the iOS runner of nsh 0.5.0
/// keeps it in memory only, so a relaunch forgot the pairing.)
const PAIRED_FILE: &str = "paired-device";

/// A parsed pairing payload. `Debug` never prints the nonce.
#[derive(Clone, PartialEq, Eq)]
pub struct Payload {
    nonce: String,
}

impl fmt::Debug for Payload {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Payload(<redacted>)")
    }
}

impl Payload {
    /// Accepts the payload as the bridge prints it, surrounded by space.
    /// The nonce is 32 lowercase hex characters.
    pub fn parse(raw: &str) -> Result<Self, WriteError> {
        let nonce = raw
            .trim()
            .strip_prefix(PAYLOAD_PREFIX)
            .ok_or(WriteError::BadPayload)?;
        let valid = nonce.len() == 32
            && nonce
                .bytes()
                .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
        if !valid {
            return Err(WriteError::BadPayload);
        }
        Ok(Self {
            nonce: nonce.to_owned(),
        })
    }
}

/// Approve or Deny a blocked agent: the key the bridge sends to its pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Approve,
    Deny,
}

impl Decision {
    pub fn key(self) -> &'static str {
        match self {
            Self::Approve => "y",
            Self::Deny => "n",
        }
    }
}

/// `POST /v1/worktrees`. The fields and their order are the bridge's
/// `CreateWorktree`, because the bridge signs its own re-serialization of
/// the body: a field this app left out would change the signed bytes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct WorktreeRequest {
    pub workspace_id: Option<String>,
    pub cwd: Option<String>,
    pub branch: Option<String>,
    pub base: Option<String>,
    pub path: Option<String>,
    pub label: Option<String>,
    pub focus: bool,
}

impl WorktreeRequest {
    fn canonical(&self) -> String {
        serde_json::to_string(self).expect("a worktree request serializes")
    }
}

/// The app's pairing, as the screens show it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PairState {
    /// No paired device key. `reason` says why when the bridge dropped it.
    Unpaired {
        reason: Option<String>,
    },
    /// A pair request is in flight.
    Pairing,
    Paired {
        device_id: String,
    },
}

impl PairState {
    pub fn is_paired(&self) -> bool {
        matches!(self, Self::Paired { .. })
    }
}

/// Why a pair, grant or write did not happen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteError {
    /// The payload is not `pendant-pair:v1:<32 hex>`.
    BadPayload,
    /// The device key refused: cancelled, no biometric match, or missing.
    Key(KeyError),
    /// The app is not paired, or the bridge says the device is unpaired or
    /// revoked. The app has dropped its pairing.
    Unpaired,
    /// No grant is open: open one first.
    GrantRequired,
    /// The grant expired or was released: open a new one.
    GrantReleased,
    /// Another device is paired and takeover is off.
    TakeoverOff,
    /// Any other refusal, with the bridge's status and code.
    Refused { status: u16, code: Option<String> },
    /// No answer, or one the app does not understand.
    Net(NetError),
}

impl fmt::Display for WriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadPayload => f.write_str("that is not a Pendant pairing code"),
            Self::Key(err) => write!(f, "{err}"),
            Self::Unpaired => f.write_str("this device is not paired"),
            Self::GrantRequired => f.write_str("open a grant first"),
            Self::GrantReleased => f.write_str("the grant ended; open a new one"),
            Self::TakeoverOff => f.write_str("another device is paired"),
            Self::Refused {
                status,
                code: Some(code),
            } => write!(f, "the bridge refused ({status} {code})"),
            Self::Refused { status, code: None } => write!(f, "the bridge refused ({status})"),
            Self::Net(err) => write!(f, "{err}"),
        }
    }
}

impl From<NetError> for WriteError {
    fn from(err: NetError) -> Self {
        match err {
            NetError::Status { status, code } => match code.as_deref() {
                Some("unpaired" | "revoked") => Self::Unpaired,
                Some("grant_required") => Self::GrantRequired,
                Some("grant_released") => Self::GrantReleased,
                Some("takeover_off") => Self::TakeoverOff,
                _ => Self::Refused { status, code },
            },
            other => Self::Net(other),
        }
    }
}

// The messages the device key signs, byte for byte the bridge's.

pub fn pair_message(device_id: &str, public_b64: &str, nonce: &str) -> Vec<u8> {
    format!("pendant-pair:v1\n{device_id}\n{public_b64}\n{nonce}").into_bytes()
}

pub fn grant_message(device_id: &str, challenge: &str) -> Vec<u8> {
    format!("pendant-grant:v1\n{device_id}\n{challenge}").into_bytes()
}

pub fn sign_message(
    device_id: &str,
    method: &str,
    path: &str,
    challenge: &str,
    canonical: &str,
) -> Vec<u8> {
    format!("pendant-sign:v1\n{device_id}\n{method}\n{path}\n{challenge}\n{canonical}").into_bytes()
}

/// The device id for a public key: stable for the key, so it needs no
/// storage of its own. The key is a random point, so its X coordinate's
/// first 8 bytes are as unique as a hash of it would be.
pub fn device_id(public_key: &[u8]) -> String {
    let x = public_key.get(1..9).unwrap_or(public_key);
    let hex: String = x.iter().map(|b| format!("{b:02x}")).collect();
    format!("pendant-{hex}")
}

/// The path segment of an agent id, as [`Bridge::agent`] encodes it.
fn agent_path(id: &str, action: &str) -> String {
    format!("/v1/agents/{}/{action}", super::encode_segment(id))
}

/// The device key's public half and id, made if there is none yet.
struct Identity {
    device_id: String,
    public_b64: String,
}

/// Runs `f` on a thread of its own and resolves with its result, so a
/// biometric prompt never blocks the engine thread.
pub(crate) fn off_thread<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> OffThread<T> {
    let slot = Arc::new(Mutex::new(Slot {
        value: None,
        waker: None,
    }));
    let worker = Arc::clone(&slot);
    let spawned = std::thread::Builder::new()
        .name("pendant-key".into())
        .spawn(move || {
            let value = f();
            let waker = {
                let mut slot = worker.lock().expect("key slot");
                slot.value = Some(value);
                slot.waker.take()
            };
            if let Some(waker) = waker {
                waker.wake();
            }
        });
    OffThread {
        slot,
        failed: spawned.is_err(),
    }
}

struct Slot<T> {
    value: Option<T>,
    waker: Option<Waker>,
}

struct OffThread<T> {
    slot: Arc<Mutex<Slot<T>>>,
    failed: bool,
}

impl<T> Future for OffThread<T> {
    type Output = Option<T>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<T>> {
        if self.failed {
            return Poll::Ready(None);
        }
        let mut slot = self.slot.lock().expect("key slot");
        match slot.value.take() {
            Some(value) => Poll::Ready(Some(value)),
            None => {
                slot.waker = Some(cx.waker().clone());
                Poll::Pending
            }
        }
    }
}

async fn identity() -> Result<Identity, WriteError> {
    off_thread(|| {
        let key = match DeviceKey::load()? {
            Some(key) => key,
            None => DeviceKey::create()?,
        };
        Ok::<_, KeyError>(Identity {
            device_id: device_id(key.public_key()),
            public_b64: STANDARD.encode(key.public_key()),
        })
    })
    .await
    .unwrap_or(Err(KeyError::Os(-1)))
    .map_err(WriteError::Key)
}

async fn sign(message: Vec<u8>, reason: &'static str) -> Result<String, WriteError> {
    off_thread(move || {
        let key = DeviceKey::load()?.ok_or(KeyError::NotFound)?;
        key.sign(&message, reason)
    })
    .await
    .unwrap_or(Err(KeyError::Os(-1)))
    .map(|der| STANDARD.encode(der))
    .map_err(WriteError::Key)
}

impl Bridge {
    /// `GET /v1/challenge`.
    pub async fn challenge(&self) -> Result<String, NetError> {
        #[derive(serde::Deserialize)]
        struct Challenge {
            challenge: String,
        }
        let reply: Challenge = self.get_json("/v1/challenge").await?;
        Ok(reply.challenge)
    }

    async fn post_json(
        &self,
        path: &str,
        body: String,
        signed: Option<(&str, &str, &str)>,
    ) -> Result<serde_json::Value, NetError> {
        let mut request = HttpRequest::post(self.url(path), body.into_bytes())
            .header("Content-Type", "application/json")
            .header("Accept", "application/json");
        if let Some((device, signature, challenge)) = signed {
            request = request
                .header("x-pendant-device", device)
                .header("x-pendant-signature", signature)
                .header("x-pendant-challenge", challenge);
        }
        let response = fetch(request).await.map_err(NetError::Transport)?;
        super::decode(response.status(), &response.text())
    }
}

/// Pairs this device from the payload. Biometric: once, for the pair
/// signature.
pub async fn pair(bridge: &Bridge, payload: &Payload) -> Result<String, WriteError> {
    let me = identity().await?;
    let message = pair_message(&me.device_id, &me.public_b64, &payload.nonce);
    let sig = sign(message, "Pair this device with Pendant").await?;
    let body = serde_json::json!({
        "device_id": me.device_id,
        "public_key": me.public_b64,
        "nonce": payload.nonce,
        "sig": sig,
    });
    bridge.post_json("/v1/pair", body.to_string(), None).await?;
    Ok(me.device_id)
}

/// Opens a grant. Biometric: once.
pub async fn open_grant(bridge: &Bridge, device_id: &str) -> Result<(), WriteError> {
    let challenge = bridge.challenge().await?;
    let sig = sign(
        grant_message(device_id, &challenge),
        "Allow Pendant to act for 30 seconds",
    )
    .await?;
    let body = serde_json::json!({
        "device_id": device_id,
        "sig": sig,
        "challenge": challenge,
    });
    bridge
        .post_json("/v1/grant", body.to_string(), None)
        .await?;
    Ok(())
}

/// One signed write. Biometric: once, for this write's signature.
async fn write(
    bridge: &Bridge,
    device_id: &str,
    path: &str,
    canonical: &str,
    body: String,
    reason: &'static str,
) -> Result<serde_json::Value, WriteError> {
    let challenge = bridge.challenge().await?;
    let message = sign_message(device_id, "POST", path, &challenge, canonical);
    let sig = sign(message, reason).await?;
    Ok(bridge
        .post_json(path, body, Some((device_id, &sig, &challenge)))
        .await?)
}

pub async fn prompt(
    bridge: &Bridge,
    device_id: &str,
    agent: &str,
    text: &str,
) -> Result<serde_json::Value, WriteError> {
    let text = text.trim();
    let body = serde_json::json!({ "prompt": text }).to_string();
    write(
        bridge,
        device_id,
        &agent_path(agent, "prompt"),
        text,
        body,
        "Send a prompt",
    )
    .await
}

pub async fn decide(
    bridge: &Bridge,
    device_id: &str,
    agent: &str,
    decision: Decision,
) -> Result<serde_json::Value, WriteError> {
    let key = decision.key();
    let body = serde_json::json!({ "decision": key }).to_string();
    let reason = match decision {
        Decision::Approve => "Approve",
        Decision::Deny => "Deny",
    };
    write(
        bridge,
        device_id,
        &agent_path(agent, "approve"),
        key,
        body,
        reason,
    )
    .await
}

pub async fn worktree_create(
    bridge: &Bridge,
    device_id: &str,
    request: &WorktreeRequest,
) -> Result<serde_json::Value, WriteError> {
    let canonical = request.canonical();
    write(
        bridge,
        device_id,
        "/v1/worktrees",
        &canonical,
        canonical.clone(),
        "Create a worktree",
    )
    .await
}

// The pairing kept between launches.

/// The paired device id kept from an earlier launch, if any.
pub(crate) fn stored_device() -> Option<String> {
    let path = app_dir()?.join(PAIRED_FILE);
    let id = std::fs::read_to_string(path).ok()?;
    valid_device_id(id.trim()).then(|| id.trim().to_owned())
}

/// Keeps (`Some`) or forgets (`None`) the paired device id.
pub(crate) fn store_device(device_id: Option<&str>) {
    let Some(dir) = app_dir() else {
        return;
    };
    let path = dir.join(PAIRED_FILE);
    match device_id {
        Some(id) => {
            // Written beside, then renamed: a crash leaves the old file or
            // the new one, never half of one.
            let partial = dir.join(format!("{PAIRED_FILE}.partial"));
            let kept = std::fs::create_dir_all(&dir)
                .and_then(|()| std::fs::write(&partial, id))
                .and_then(|()| std::fs::rename(&partial, &path));
            if let Err(err) = kept {
                super::live::trace(&format!("pairing not kept: {}", err.kind()));
            }
        }
        None => {
            let _ = std::fs::remove_file(path);
        }
    }
}

fn valid_device_id(id: &str) -> bool {
    id.strip_prefix("pendant-")
        .is_some_and(|hex| hex.len() == 16 && hex.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// The app's own data directory: nsh's when the host gives one, else the
/// platform's per-app place.
fn app_dir() -> Option<std::path::PathBuf> {
    data_dir().or_else(platform_dir)
}

#[cfg(target_os = "ios")]
fn platform_dir() -> Option<std::path::PathBuf> {
    // An iOS app's HOME is its own data container.
    let home = std::env::var_os("HOME")?;
    Some(std::path::Path::new(&home).join("Library/Application Support/Pendant"))
}

#[cfg(target_os = "android")]
fn platform_dir() -> Option<std::path::PathBuf> {
    // The process name is the package; its private files directory.
    let cmdline = std::fs::read("/proc/self/cmdline").ok()?;
    let package = cmdline.split(|b| *b == 0).next()?;
    let package = std::str::from_utf8(package).ok()?;
    let valid = !package.is_empty()
        && package
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_');
    valid.then(|| std::path::PathBuf::from(format!("/data/data/{package}/files")))
}

#[cfg(not(any(target_os = "ios", target_os = "android")))]
fn platform_dir() -> Option<std::path::PathBuf> {
    None
}

#[cfg(test)]
mod tests;
