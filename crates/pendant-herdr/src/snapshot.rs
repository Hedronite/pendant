use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Snapshot {
    pub raw: Value,
    pub agents: Vec<AgentRecord>,
    pub workspaces: Vec<WorkspaceRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentRecord {
    /// Public target: live agent `name` if present, otherwise `pane_id`.
    pub id: String,
    pub name: Option<String>,
    pub agent: Option<String>,
    pub status: String,
    pub pane_id: String,
    pub workspace_id: String,
    pub tab_id: Option<String>,
    pub title: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkspaceRecord {
    pub workspace_id: String,
    pub label: Option<String>,
    pub cwd: Option<String>,
    pub worktree: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InboxItem {
    pub id: String,
    pub agent: Option<String>,
    pub status: String,
    pub workspace_id: String,
    pub title: Option<String>,
    pub kind: String,
}

impl Snapshot {
    pub fn from_raw(raw: Value) -> Self {
        let root = unwrap_snapshot_root(&raw);
        let agents = extract_agents(root);
        let workspaces = extract_workspaces(root);
        Self {
            raw,
            agents,
            workspaces,
        }
    }

    pub fn inbox(&self) -> Vec<InboxItem> {
        self.agents
            .iter()
            .filter(|agent| agent.status == "blocked" || agent.status == "done")
            .map(|agent| InboxItem {
                id: agent.id.clone(),
                agent: agent.agent.clone().or_else(|| agent.name.clone()),
                status: agent.status.clone(),
                workspace_id: agent.workspace_id.clone(),
                title: agent.title.clone(),
                kind: agent.status.clone(),
            })
            .collect()
    }

    pub fn agent_by_id(&self, id: &str) -> Option<&AgentRecord> {
        self.agents.iter().find(|agent| {
            agent.id == id || agent.pane_id == id || agent.name.as_deref() == Some(id)
        })
    }
}

fn unwrap_snapshot_root(raw: &Value) -> &Value {
    let root = raw.get("result").unwrap_or(raw);
    // herdr 0.9.x nests the payload once more: {result: {snapshot: {...}}}.
    root.get("snapshot").unwrap_or(root)
}

fn extract_agents(root: &Value) -> Vec<AgentRecord> {
    let Some(items) = root.get("agents").and_then(Value::as_array) else {
        return Vec::new();
    };
    items.iter().filter_map(parse_agent).collect()
}

fn parse_agent(value: &Value) -> Option<AgentRecord> {
    let pane_id = string_field(value, &["pane_id"]).unwrap_or_default();
    let name = string_field(value, &["name"]);
    let id = name
        .clone()
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| pane_id.clone());
    if id.is_empty() && pane_id.is_empty() {
        return None;
    }
    let title = string_field(
        value,
        &["title", "display_agent", "terminal_title_stripped"],
    )
    .or_else(|| {
        value
            .get("titles")
            .and_then(Value::as_array)
            .and_then(|titles| titles.first())
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
    });
    Some(AgentRecord {
        id,
        name,
        agent: string_field(value, &["agent"]),
        status: string_field(value, &["agent_status", "status"])
            .unwrap_or_else(|| "unknown".into()),
        pane_id,
        workspace_id: string_field(value, &["workspace_id"]).unwrap_or_default(),
        tab_id: string_field(value, &["tab_id"]),
        title,
    })
}

fn extract_workspaces(root: &Value) -> Vec<WorkspaceRecord> {
    let Some(items) = root.get("workspaces").and_then(Value::as_array) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|value| {
            let workspace_id = string_field(value, &["workspace_id", "id"])?;
            Some(WorkspaceRecord {
                workspace_id,
                label: string_field(value, &["label", "name"]),
                cwd: string_field(value, &["cwd"]),
                worktree: value.get("worktree").cloned(),
            })
        })
        .collect()
}

fn string_field(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        value
            .get(*key)
            .and_then(Value::as_str)
            .map(str::to_string)
            .filter(|s| !s.is_empty())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn snapshot_extracts_agents_and_inbox() {
        let raw = json!({
            "type": "session_snapshot",
            "agents": [
                {
                    "name": "reviewer",
                    "agent": "codex",
                    "agent_status": "blocked",
                    "pane_id": "w1:p1",
                    "workspace_id": "w1",
                    "tab_id": "w1:t1",
                    "title": "needs approve"
                },
                {
                    "agent": "claude",
                    "agent_status": "idle",
                    "pane_id": "w1:p2",
                    "workspace_id": "w1"
                }
            ],
            "workspaces": [{ "workspace_id": "w1", "label": "api", "cwd": "/repo" }]
        });
        let snap = Snapshot::from_raw(raw);
        assert_eq!(snap.agents.len(), 2);
        assert_eq!(snap.agents[0].id, "reviewer");
        assert_eq!(snap.workspaces[0].label.as_deref(), Some("api"));
        let inbox = snap.inbox();
        assert_eq!(inbox.len(), 1);
        assert_eq!(inbox[0].kind, "blocked");
    }

    #[test]
    fn snapshot_unwraps_result_snapshot_envelope() {
        // Real herdr 0.9.0 `api snapshot` shape: {result: {snapshot: {...}}}.
        let raw = json!({
            "id": "cli:api:snapshot",
            "result": {
                "snapshot": {
                    "agents": [{
                        "agent": "omp",
                        "agent_status": "working",
                        "pane_id": "wN:p2",
                        "workspace_id": "wN",
                        "tab_id": "wN:t1",
                        "terminal_title_stripped": "Pendant spike"
                    }],
                    "workspaces": [{ "workspace_id": "wN", "label": "halo" }]
                }
            }
        });
        let snap = Snapshot::from_raw(raw);
        assert_eq!(snap.agents.len(), 1);
        assert_eq!(snap.agents[0].pane_id, "wN:p2");
        assert_eq!(snap.agents[0].status, "working");
        assert_eq!(snap.agents[0].title.as_deref(), Some("Pendant spike"));
        assert_eq!(snap.workspaces.len(), 1);
    }
}
