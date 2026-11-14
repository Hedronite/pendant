//! The agent screens. Frontend owns this file; the contract from the
//! scaffold:
//!
//! - `list(nav)` is the bottom page; `nav.push(Route::Agent(id))` opens one
//!   agent and `nav.push(Route::Inbox)` the inbox.
//! - `detail(nav, id)` and `inbox(nav)` are pushed pages.
//!
//! Data comes from the live link ([`crate::net::live`], the shared
//! snapshot/inbox/pane signals fullstack refreshes) and, for an agent's
//! recent output, [`crate::bridge`]. No screen holds a client of its own.

mod detail;
mod inbox;
mod list;

pub use detail::detail;
pub use inbox::inbox;
pub use list::list;

/// The pairing page. A placeholder from the shell for frontend to replace:
/// it keeps `lib.rs` building with `Route::Pair`. The screen takes the
/// payload pasted (or `payload`, from a link) and calls
/// `net::live::live().pair(..)` / `.pair_with(..)`.
pub fn pairing(_nav: crate::Nav, _payload: Option<crate::net::pair::Payload>) -> Screen {
    nsh_base::nav_page()
        .title("Pair")
        .child(view().flex_1().p_4().child(text("Pair this device")))
}

#[cfg(test)]
mod tests;

use nsh_core::prelude::*;

use crate::theme;
use crate::Nav;

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

/// The link state one line, for the bottom of a page's header slot and the
/// top of a page's content: what the WebSocket is doing right now.
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
