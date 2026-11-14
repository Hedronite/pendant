//! The bridge's JSON, as the app reads it. Field names follow the bridge on
//! `main` (`crates/pendant-herdr/src/snapshot.rs` and
//! `crates/pendant-bridge/src/routes.rs`). Unknown fields are ignored, so a
//! bridge that adds a field does not break an older app.

use serde::Deserialize;
use serde_json::Value;

/// `GET /v1/snapshot`. The bridge also sends `raw`, herdr's own reply,
/// which the app does not read.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Snapshot {
    pub agents: Vec<Agent>,
    #[serde(default)]
    pub workspaces: Vec<Workspace>,
}

impl Snapshot {
    /// The agents whose status is `blocked`.
    pub fn blocked(&self) -> impl Iterator<Item = &Agent> {
        self.agents.iter().filter(|agent| agent.is_blocked())
    }

    /// The agent the bridge would resolve for `id`: its public id, its
    /// pane id or its name.
    pub fn agent(&self, id: &str) -> Option<&Agent> {
        self.agents.iter().find(|agent| {
            agent.id == id || agent.pane_id == id || agent.name.as_deref() == Some(id)
        })
    }
}

/// One agent in a snapshot (`AgentRecord` on the bridge).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Agent {
    /// The public target: the agent's name if it has one, else its pane id.
    pub id: String,
    pub name: Option<String>,
    /// The harness, such as `claude`.
    pub agent: Option<String>,
    /// herdr's status: `working`, `idle`, `blocked`, `done`, …
    pub status: String,
    pub pane_id: String,
    pub workspace_id: String,
    pub tab_id: Option<String>,
    pub title: Option<String>,
}

impl Agent {
    pub fn is_blocked(&self) -> bool {
        self.status == "blocked"
    }

    /// What a list row shows: the title, else the name, else the id.
    pub fn label(&self) -> &str {
        self.title
            .as_deref()
            .or(self.name.as_deref())
            .filter(|label| !label.is_empty())
            .unwrap_or(&self.id)
    }
}

/// One workspace in a snapshot (`WorkspaceRecord` on the bridge).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Workspace {
    pub workspace_id: String,
    pub label: Option<String>,
    pub cwd: Option<String>,
    pub worktree: Option<Value>,
}

/// `GET /v1/agents/{id}`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct AgentDetail {
    pub id: String,
    /// `null` when the snapshot no longer has the agent.
    pub agent: Option<Agent>,
    /// herdr's `agent.get` reply, as is.
    pub get: Option<Value>,
    /// herdr's `agent.read` reply (the recent lines), as is.
    pub output: Option<Value>,
    #[serde(default)]
    pub blocked: bool,
}

impl AgentDetail {
    /// The recent pane text in `output`, wherever herdr put it.
    pub fn output_text(&self) -> Option<&str> {
        let output = self.output.as_ref()?;
        ["/read/text", "/text", "/result/read/text", "/result/text"]
            .iter()
            .find_map(|pointer| output.pointer(pointer).and_then(Value::as_str))
    }
}

/// `GET /v1/events`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Inbox {
    pub items: Vec<InboxItem>,
}

/// One inbox event: an agent that is `blocked` or `done`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct InboxItem {
    pub id: String,
    pub agent: Option<String>,
    pub status: String,
    pub workspace_id: String,
    pub title: Option<String>,
    /// The event kind; today the same as `status`.
    pub kind: String,
}

impl InboxItem {
    pub fn is_blocked(&self) -> bool {
        self.kind == "blocked" || self.status == "blocked"
    }
}

/// The body of a refusal: `{"error": {"code", "message"}}`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ErrorBody {
    pub error: ErrorDetail,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ErrorDetail {
    pub code: String,
    #[serde(default)]
    pub message: String,
}
