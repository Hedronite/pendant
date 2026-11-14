//! The bridge's WebSocket envelope (`/v1/ws`, version 1), as in
//! `crates/pendant-bridge/src/session.rs`. The app sends [`Up`] messages and
//! reads [`Down`] ones. Writes (`input`, `resize`, `scroll`) stay refused by
//! the bridge, and the app never sends them.

use serde::{Deserialize, Serialize};

/// The envelope version the bridge speaks.
pub const VERSION: u32 = 1;

/// The bridge's ceiling on watched panes per socket.
pub const MAX_WATCHES: usize = 8;

/// A message from the app to the bridge.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Up {
    pub v: u32,
    pub pane: String,
    #[serde(rename = "type")]
    pub kind: UpKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cols: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rows: Option<u16>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum UpKind {
    Watch,
    Unwatch,
    Resync,
}

impl Up {
    /// Start frames for `pane`. The bridge first sends its scrollback.
    pub fn watch(pane: impl Into<String>, cols: u16, rows: u16) -> Self {
        Self {
            v: VERSION,
            pane: pane.into(),
            kind: UpKind::Watch,
            cols: Some(cols),
            rows: Some(rows),
        }
    }

    pub fn unwatch(pane: impl Into<String>) -> Self {
        Self::bare(pane, UpKind::Unwatch)
    }

    /// Ask again for the last full frame of `pane`.
    pub fn resync(pane: impl Into<String>) -> Self {
        Self::bare(pane, UpKind::Resync)
    }

    fn bare(pane: impl Into<String>, kind: UpKind) -> Self {
        Self {
            v: VERSION,
            pane: pane.into(),
            kind,
            cols: None,
            rows: None,
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("an envelope always serializes")
    }
}

/// A message from the bridge.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Down {
    /// A terminal frame. `seq` rises per pane; `bytes` is base64 ANSI.
    Frame(Frame),
    /// The pane's recent text, sent once on `watch`.
    Scrollback { v: u32, pane: String, text: String },
    /// The bridge refused `op` for `pane`.
    Refused { v: u32, pane: String, op: String },
    /// A type this app does not know yet.
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Frame {
    pub v: u32,
    pub pane: String,
    pub full: bool,
    pub seq: u64,
    pub bytes: String,
    pub width: u32,
    pub height: u32,
}

impl Down {
    /// Parses one text message. `None` for a message that is not JSON or
    /// carries another envelope version.
    pub fn parse(text: &str) -> Option<Self> {
        let down: Self = serde_json::from_str(text).ok()?;
        match &down {
            Self::Frame(Frame { v, .. }) | Self::Scrollback { v, .. } | Self::Refused { v, .. }
                if *v != VERSION =>
            {
                None
            }
            _ => Some(down),
        }
    }
}

/// Tracks `seq` per pane, to show that frames keep coming and to spot a gap.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SeqTrack {
    last: Option<u64>,
    /// Frames seen.
    pub frames: u64,
    /// Frames whose `seq` was not above the one before.
    pub out_of_order: u64,
}

impl SeqTrack {
    pub fn observe(&mut self, seq: u64) {
        if self.last.is_some_and(|last| seq <= last) {
            self.out_of_order += 1;
        }
        self.last = Some(self.last.map_or(seq, |last| last.max(seq)));
        self.frames += 1;
    }

    pub fn last(&self) -> Option<u64> {
        self.last
    }
}
