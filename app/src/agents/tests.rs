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

#[test]
fn write_errors_carry_their_own_copy_and_severity() {
    use super::note_for;
    use crate::keystore::KeyError;
    use crate::net::pair::WriteError;

    // A refusal the user must act on: Gold.
    let (text, color) = note_for(&WriteError::Unpaired);
    assert_eq!(text, "this device is not paired");
    assert_eq!(color, crate::theme::GOLD);

    // A wrong payload is refused in Gold, in the screen's own words.
    let (text, color) = note_for(&WriteError::BadPayload);
    assert_eq!(text, "that is not a Pendant pairing code");
    assert_eq!(color, crate::theme::GOLD);

    // Grant hints stay quiet: the next write opens one.
    let (_, color) = note_for(&WriteError::GrantRequired);
    assert_eq!(color, crate::theme::DIM);
    let (_, color) = note_for(&WriteError::GrantReleased);
    assert_eq!(color, crate::theme::DIM);

    // A cancelled biometric is the user's own action, not a fault.
    let (text, color) = note_for(&WriteError::Key(KeyError::Cancelled));
    assert_eq!(text, "biometric cancelled; try again");
    assert_eq!(color, crate::theme::DIM);

    // A failed biometric match is Gold.
    let (_, color) = note_for(&WriteError::Key(KeyError::AuthFailed));
    assert_eq!(color, crate::theme::GOLD);
}

#[test]
fn the_worktree_stamp_is_six_digits() {
    use super::detail::wt_stamp;
    let stamp = wt_stamp();
    assert_eq!(stamp.len(), 6);
    assert!(stamp.bytes().all(|b| b.is_ascii_digit()), "{stamp}");
}
