//! The agent screens. Frontend owns this file; the contract from the
//! scaffold:
//!
//! - `list(nav)` is the bottom page; `nav.push(Route::Agent(id))` opens one
//!   agent and `nav.push(Route::Inbox)` the inbox.
//! - `detail(nav, id)` and `inbox(nav)` are pushed pages.
//!
//! Data comes from [`crate::bridge`] and [`crate::net`]; the screens hold no
//! client of their own. WebSocket inbox updates wait on a PM ruling; the
//! inbox reads `Bridge::inbox()`.

mod detail;
mod inbox;
mod list;

pub use detail::detail;
pub use inbox::inbox;
pub use list::list;

#[cfg(test)]
mod tests;

use nsh_core::prelude::*;

use crate::theme;

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
pub(crate) fn status_pill(status: impl Fn() -> SharedString + Copy + 'static) -> impl IntoElement {
    view()
        .flex_row()
        .items_center()
        .rounded(px(999.0))
        .px(px(10.0))
        .py(px(3.0))
        .bg(move || {
            let (bg, _) = theme::status_pill(&status());
            rgb(bg)
        })
        .child(
            text(status)
                .text_size(px(12.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(move || {
                    let (_, fg) = theme::status_pill(&status());
                    rgb(fg)
                }),
        )
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
        .child(
            text(error)
                .text_size(px(13.0))
                .text_color(rgb(theme::DIM)),
        )
        .child(
            text("Retry")
                .text_size(px(15.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(theme::TEAL))
                .on_press(retry)
                .role(AccessibilityRole::Button),
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
        .child(
            when(
                move || nav.depth() > 1,
                move || {
                    text("Back")
                        .text_size(px(15.0))
                        .text_color(rgb(theme::TEAL))
                        .on_press(move || nav.back())
                        .role(AccessibilityRole::Button)
                },
            ),
        )
        .child(
            text(title)
                .text_size(px(17.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(theme::TEXT)),
        )
}
