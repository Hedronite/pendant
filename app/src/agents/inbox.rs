//! The inbox: agents that are blocked or done, newest first.

use nsh_base::nav_page;
use nsh_core::prelude::*;

use crate::agents::{error_line, event_label, page_header, status_pill};
use crate::net::live::live;
use crate::theme;
use crate::{Nav, Route};

/// The inbox, pushed with `Route::Inbox`. The live link refreshes the
/// events every few seconds; WebSocket-driven updates wait on a PM ruling
/// (SPEC-shell G1).
pub fn inbox(nav: Nav) -> Screen {
    let live = live();

    nav_page()
        .title("Inbox")
        .header(page_header("Inbox", nav))
        .child(
            view()
                .flex_1()
                .flex_col()
                .bg(rgb(theme::GRAPHITE))
                .child(when(
                    move || live.error.get().is_some(),
                    move || error_line(move || live.error.get().unwrap_or_default().into()),
                ))
                .child(
                    virtual_list(
                        move || live.inbox.with(Vec::len),
                        move |at| {
                            view()
                                .min_h(px(56.0))
                                .flex_row()
                                .items_center()
                                .gap_3()
                                .px_4()
                                .py_2()
                                .on_press(move || {
                                    let id = live.inbox.with(|items| {
                                        items.get(at.index()).map(|item| item.id.clone())
                                    });
                                    if let Some(id) = id {
                                        nav.push(Route::Agent(id));
                                    }
                                })
                                .child(
                                    // The agent the event is about.
                                    text(move || {
                                        live.inbox
                                            .with(|items| items.get(at.index()).map(event_label))
                                            .unwrap_or_default()
                                    })
                                    .text_size(px(16.0))
                                    .text_color(rgb(theme::TEXT))
                                    .number_of_lines(1)
                                    .flex_1(),
                                )
                                .child(status_pill(move || {
                                    live.inbox
                                        .with(|items| {
                                            items.get(at.index()).map(|item| item.kind.clone())
                                        })
                                        .unwrap_or_default()
                                        .into()
                                }))
                        },
                    )
                    .estimated_item_size(px(64.0))
                    .keys(move || {
                        live.inbox.with(|items| {
                            items.iter().map(|item| item.id.clone()).collect::<Vec<_>>()
                        })
                    })
                    .separator(move || view().h(px(1.0)).mx_4().bg(rgb(theme::LINE)))
                    .empty(
                        view().p_8().items_center().child(
                            text(move || match live.link.get() {
                                crate::net::live::Link::Connecting => "Connecting…".to_owned(),
                                _ => "Nothing needs you".to_owned(),
                            })
                            .text_size(px(15.0))
                            .text_color(rgb(theme::DIM)),
                        ),
                    )
                    .flex_1(),
                ),
        )
}
