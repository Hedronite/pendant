//! The screens' own logic, against JSON shaped like the bridge's replies.

use super::event_label;
use crate::net::InboxItem;

fn event(id: &str, agent: Option<&str>, title: Option<&str>) -> InboxItem {
    InboxItem {
        id: id.to_owned(),
        agent: agent.map(str::to_owned),
        status: "blocked".to_owned(),
        workspace_id: "w1".to_owned(),
        title: title.map(str::to_owned),
        kind: "blocked".to_owned(),
    }
}

#[test]
fn an_event_label_prefers_the_title() {
    let item = event("w1:p2", Some("scratch-2"), Some("Needs your OK"));
    assert_eq!(event_label(&item), "Needs your OK");
}

#[test]
fn an_event_label_falls_back_from_title_to_agent_to_id() {
    let item = event("w1:p2", Some("scratch-2"), Some(""));
    assert_eq!(event_label(&item), "scratch-2");

    let bare = event("w1:p2", None, None);
    assert_eq!(event_label(&bare), "w1:p2");
}
