//! The app's line to the bridge: HTTP through nsh's `fetch` (the platform's
//! own stack: NSURLSession on iOS, HttpURLConnection on Android) and the
//! WebSocket in [`live`] (no async runtime of its own).
//!
//! The bridge's address is not in the source. It comes from
//! `PENDANT_BRIDGE_URL` when the app is built, for example
//! `PENDANT_BRIDGE_URL=https://bridge.example.ts.net:8443 nsh package …`.

pub mod live;
pub mod types;
pub mod ws;

pub use types::{Agent, AgentDetail, Inbox, InboxItem, Snapshot, Workspace};

use nsh_core::prelude::*;
use serde::de::DeserializeOwned;

/// The variable read at build time.
pub const BRIDGE_URL_VAR: &str = "PENDANT_BRIDGE_URL";

/// The bridge's origin, checked: `https://host[:port]`, no path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bridge {
    origin: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    /// The app was built without `PENDANT_BRIDGE_URL`.
    Unset,
    /// Not `https://host[:port]`.
    Invalid(String),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unset => write!(f, "this build has no bridge address ({BRIDGE_URL_VAR})"),
            Self::Invalid(why) => write!(f, "the bridge address is not usable: {why}"),
        }
    }
}

impl Bridge {
    /// The bridge this build was made for.
    pub fn from_build() -> Result<Self, ConfigError> {
        Self::parse(option_env!("PENDANT_BRIDGE_URL").ok_or(ConfigError::Unset)?)
    }

    /// Accepts `https://host` or `https://host:port`, with an optional
    /// trailing slash. Plain `http`, a path, a query or user info is refused.
    pub fn parse(raw: &str) -> Result<Self, ConfigError> {
        let raw = raw.trim();
        let rest = raw
            .strip_prefix("https://")
            .ok_or_else(|| ConfigError::Invalid("it must start with https://".into()))?;
        let authority = rest.strip_suffix('/').unwrap_or(rest);
        if authority.is_empty() {
            return Err(ConfigError::Invalid("it has no host".into()));
        }
        if authority.contains(['/', '?', '#', '@']) || authority.chars().any(char::is_whitespace) {
            return Err(ConfigError::Invalid("it must be an origin only".into()));
        }
        let (host, port) = match authority.rsplit_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (authority, None),
        };
        if host.is_empty() || host.contains(':') {
            return Err(ConfigError::Invalid("it has no usable host".into()));
        }
        if let Some(port) = port {
            if port.parse::<u16>().map_or(true, |port| port == 0) {
                return Err(ConfigError::Invalid("its port is not a number".into()));
            }
        }
        Ok(Self {
            origin: format!("https://{}", authority.to_ascii_lowercase()),
        })
    }

    /// `https://host[:port]`, also the `Origin` the bridge's `/v1/ws` wants.
    pub fn origin(&self) -> &str {
        &self.origin
    }

    /// `wss://host[:port]/v1/ws`.
    pub fn ws_url(&self) -> String {
        format!("wss://{}/v1/ws", self.authority())
    }

    /// The host and port to dial (443 when the origin names none).
    pub fn host_port(&self) -> (&str, u16) {
        let authority = self.authority();
        match authority.rsplit_once(':') {
            Some((host, port)) => (host, port.parse().unwrap_or(443)),
            None => (authority, 443),
        }
    }

    fn authority(&self) -> &str {
        &self.origin["https://".len()..]
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.origin)
    }

    /// `GET /v1/snapshot`.
    pub async fn snapshot(&self) -> Result<Snapshot, NetError> {
        self.get_json("/v1/snapshot").await
    }

    /// `GET /v1/agents/{id}`.
    pub async fn agent(&self, id: &str) -> Result<AgentDetail, NetError> {
        self.get_json(&format!("/v1/agents/{}", encode_segment(id)))
            .await
    }

    /// `GET /v1/events`: the inbox.
    pub async fn inbox(&self) -> Result<Inbox, NetError> {
        self.get_json("/v1/events").await
    }

    async fn get_json<T: DeserializeOwned>(&self, path: &str) -> Result<T, NetError> {
        let response = fetch(HttpRequest::get(self.url(path)).header("Accept", "application/json"))
            .await
            .map_err(NetError::Transport)?;
        decode(response.status(), &response.text())
    }
}

/// Why a call to the bridge failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetError {
    /// No HTTP response: no route, DNS, TLS, or a refused capability.
    Transport(String),
    /// The bridge answered with an error status, and its code if it gave one.
    Status { status: u16, code: Option<String> },
    /// The body was not the JSON this app expects.
    Decode(String),
}

impl std::fmt::Display for NetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transport(why) => write!(f, "cannot reach the bridge: {why}"),
            Self::Status {
                status,
                code: Some(code),
            } => write!(f, "the bridge said {status} ({code})"),
            Self::Status { status, code: None } => write!(f, "the bridge said {status}"),
            Self::Decode(why) => write!(f, "unexpected reply from the bridge: {why}"),
        }
    }
}

/// Turns a status and body into `T`, or the bridge's error code.
pub fn decode<T: DeserializeOwned>(status: u16, body: &str) -> Result<T, NetError> {
    if !(200..300).contains(&status) {
        let code = serde_json::from_str::<types::ErrorBody>(body)
            .ok()
            .map(|body| body.error.code);
        return Err(NetError::Status { status, code });
    }
    serde_json::from_str(body).map_err(|err| NetError::Decode(err.to_string()))
}

/// Percent-encodes one path segment (agent ids hold `:`, which is fine, but
/// never `/` or `?` unescaped).
fn encode_segment(segment: &str) -> String {
    let mut out = String::with_capacity(segment.len());
    for byte in segment.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b':' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests;
