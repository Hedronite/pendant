//! The agent screens. Frontend owns this file; the contract from the
//! scaffold:
//!
//! - `list(nav)` is the bottom page; `nav.push(Route::Agent(id))` opens one
//!   agent and `nav.push(Route::Inbox)` the inbox; `Route::Pair(payload)`
//!   the pairing page.
//! - `detail(nav, id)` and `inbox(nav)` are pushed pages.
//!
//! Data comes from the live link ([`crate::net::live`], the shared
//! snapshot/inbox/pane/pairing signals fullstack refreshes) and, for an
//! agent's recent output, [`crate::bridge`]. Every write goes through the
//! live link's signed calls; no screen holds a client of its own.

mod detail;
mod inbox;
mod list;
mod pairing;

pub use detail::detail;
pub use inbox::inbox;
pub use list::list;
pub use pairing::pairing;

#[cfg(test)]
mod tests;

use nsh_core::prelude::*;

use crate::keystore::KeyError;
use crate::net::live::live;
use crate::net::pair::{PairState, WriteError};
use crate::theme;
use crate::{Nav, Route};

/// What an inbox row shows for an event: the item's title, else the agent's
/// name, else its id.
pub(crate) fn event_label(item: &crate::net::InboxItem) -> String {
    item.title
        .clone()
        .filter(|title| !title.is_empty())
        .or_else(|| item.agent.clone())
        .unwrap_or_else(|| item.id.clone())
}

/// One status pill: a herdr status in a rounded chip, `blocked` in Gold on
/// Graphite — the treatment a row, a detail page and an inbox item share.
/// The status reads through `status` every turn, so a recycled row or a
/// refetch updates the pill in place.
pub(crate) fn status_pill(status: impl Fn() -> SharedString + Clone + 'static) -> impl IntoElement {
    let status_bg = status.clone();
    let status_text = status.clone();
    let status_fg = status.clone();
    view()
        .flex_row()
        .items_center()
        .rounded(px(999.0))
        .px(px(10.0))
        .py(px(3.0))
        .bg(move || {
            let (bg, _) = theme::status_pill(&status_bg());
            rgb(bg)
        })
        .child(
            text(move || status_text().to_string())
                .text_size(px(12.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(move || {
                    let (_, fg) = theme::status_pill(&status_fg());
                    rgb(fg)
                }),
        )
}

/// What a failed shared fetch shows. The live link retries itself every
/// [`crate::net::live::POLL`], so this is the error text only.
pub(crate) fn error_line(error: impl Fn() -> SharedString + Copy + 'static) -> impl IntoElement {
    view()
        .p_4()
        .child(text(error).text_size(px(13.0)).text_color(rgb(theme::DIM)))
}

/// What a failed load shows: the error, and Retry, which refetches.
pub(crate) fn error_note(
    error: impl Fn() -> SharedString + Copy + 'static,
    retry: impl Fn() + Copy + 'static,
) -> impl IntoElement {
    view()
        .flex_col()
        .items_center()
        .gap_2()
        .p_4()
        .child(text(error).text_size(px(13.0)).text_color(rgb(theme::DIM)))
        .child(
            text("Retry")
                .text_size(px(15.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(theme::TEAL))
                .on_press(retry)
                .role(AccessibilityRole::Button),
        )
}

/// The copy and severity a write error shows: `(text, colour)`. A refusal
/// the user caused or must act on is Gold; a hint stays quiet. The text is
/// the screen's own; [`WriteError`]'s `Display` is the fallback.
pub(crate) fn note_for(error: &WriteError) -> (String, u32) {
    match error {
        WriteError::BadPayload => ("that is not a Pendant pairing code".to_owned(), theme::GOLD),
        WriteError::Key(err) => match err {
            KeyError::Cancelled => ("biometric cancelled; try again".to_owned(), theme::DIM),
            KeyError::AuthFailed => ("the biometric did not match".to_owned(), theme::GOLD),
            KeyError::Exists | KeyError::NotFound | KeyError::Os(_) | KeyError::Unsupported(_) => {
                (err.to_string(), theme::GOLD)
            }
        },
        WriteError::Unpaired => ("this device is not paired".to_owned(), theme::GOLD),
        WriteError::GrantRequired | WriteError::GrantReleased => (
            "the grant ended; the next write opens one".to_owned(),
            theme::DIM,
        ),
        WriteError::TakeoverOff => ("another device is paired".to_owned(), theme::GOLD),
        WriteError::Refused { .. } => (error.to_string(), theme::GOLD),
        WriteError::Net(_) => (error.to_string(), theme::DIM),
    }
}

/// What a failed write shows, under the controls it came from.
pub(crate) fn write_note(
    error: impl Fn() -> Option<WriteError> + Copy + 'static,
) -> impl IntoElement {
    view().test_id("write-error").px_4().py_1().child(
        text(move || error().map(|err| note_for(&err).0).unwrap_or_default())
            .text_size(px(12.0))
            .text_color(move || {
                error()
                    .map(|err| rgb(note_for(&err).1))
                    .unwrap_or_else(|| rgb(theme::DIM))
            })
            .number_of_lines(3),
    )
}

/// How a write control looks: the primary action, or the quiet one.
pub(crate) enum ButtonKind {
    Primary,
    Quiet,
}

/// The link state one line: what the WebSocket is doing right now.
pub(crate) fn link_line(
    link: impl Fn() -> crate::net::live::Link + Copy + 'static,
) -> impl IntoElement {
    view()
        .flex_row()
        .items_center()
        .gap_2()
        .px_4()
        .py_2()
        .border_b_1()
        .border_color(rgb(theme::LINE))
        .child(
            view()
                .size(px(8.0))
                .rounded(px(4.0))
                .bg(move || match link() {
                    crate::net::live::Link::Open => rgb(theme::TEAL),
                    crate::net::live::Link::Connecting => rgb(theme::DIM),
                    _ => rgb(theme::GOLD),
                }),
        )
        .child(
            text(move || match link() {
                crate::net::live::Link::Open => "Live".to_owned(),
                crate::net::live::Link::Connecting => "Connecting…".to_owned(),
                crate::net::live::Link::Closed(why) => format!("Reconnecting: {why}"),
                crate::net::live::Link::Unconfigured(why) => format!("No bridge address: {why}"),
            })
            .text_size(px(12.0))
            .text_color(move || match link() {
                crate::net::live::Link::Open => rgb(theme::TEXT),
                crate::net::live::Link::Connecting => rgb(theme::DIM),
                _ => rgb(theme::GOLD),
            })
            .number_of_lines(1),
        )
}

/// A write control. Disabled while `enabled` says no — a busy signature or
/// an unpaired device starts nothing — and it says so by going quiet.
pub(crate) fn write_button(
    id: &'static str,
    label: &'static str,
    enabled: impl Fn() -> bool + Clone + 'static,
    press: impl Fn() + Clone + 'static,
    kind: ButtonKind,
) -> impl IntoElement {
    let (bg, fg) = match kind {
        ButtonKind::Primary => (theme::TEAL, theme::TEXT),
        ButtonKind::Quiet => (theme::SURFACE, theme::GOLD),
    };
    let press_enabled = enabled.clone();
    let color_enabled = enabled;
    view()
        .test_id(id)
        .rounded(px(10.0))
        .px_3()
        .py_2()
        .bg(rgb(bg))
        .on_press(move || {
            if press_enabled() {
                press();
            }
        })
        .role(AccessibilityRole::Button)
        .child(
            text(label)
                .text_size(px(15.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(move || rgb(if color_enabled() { fg } else { theme::DIM })),
        )
}

/// What an unpaired device shows where writes would go: why, and the way
/// to the pairing page.
pub(crate) fn unpaired_banner(nav: Nav) -> impl IntoElement {
    view()
        .flex_row()
        .items_center()
        .gap_3()
        .px_4()
        .py_2()
        .border_b_1()
        .border_color(rgb(theme::LINE))
        .bg(rgb(theme::SURFACE))
        .child(
            text(move || match live().pairing.get() {
                PairState::Unpaired {
                    reason: Some(reason),
                } => format!("Not paired: {reason}"),
                _ => "This device is not paired.".to_owned(),
            })
            .text_size(px(13.0))
            .text_color(rgb(theme::GOLD))
            .number_of_lines(2)
            .flex_1(),
        )
        .child(
            text("Pair now")
                .text_size(px(15.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(theme::TEAL))
                .on_press(move || nav.push(Route::Pair(None)))
                .role(AccessibilityRole::Button)
                .test_id("pair-now"),
        )
}

/// The page's header on the desktops and the web; iOS and Android show the
/// system header from `title`. The back button shows once the stack is more
/// than one page deep.
pub(crate) fn page_header(title: &'static str, nav: Nav) -> impl IntoElement {
    view()
        .h(px(48.0))
        .flex_row()
        .items_center()
        .gap_3()
        .border_b_1()
        .border_color(rgb(theme::LINE))
        .bg(rgb(theme::SURFACE))
        .px_4()
        .child(when(
            move || nav.depth() > 1,
            move || {
                text("Back")
                    .text_size(px(15.0))
                    .text_color(rgb(theme::TEAL))
                    .on_press(move || nav.back())
                    .role(AccessibilityRole::Button)
            },
        ))
        .child(
            text(title)
                .text_size(px(17.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(theme::TEXT)),
        )
}
