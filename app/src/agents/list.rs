//! The agent list: the app's bottom page.

use nsh_base::nav_page;
use nsh_core::prelude::*;

use crate::agents::{error_line, link_line, status_pill};
use crate::net::live::live;
use crate::theme;
use crate::{Nav, Route};

/// The header link to the inbox, in the page's header slot.
fn open_inbox(nav: Nav) -> impl IntoElement {
    text("Inbox")
        .text_size(px(15.0))
        .text_color(rgb(theme::TEAL))
        .on_press(move || nav.push(Route::Inbox))
        .role(AccessibilityRole::Button)
}

/// The agent list, the app's bottom page. The link state and the agent
/// count sit above the list; the snapshot refreshes itself every few
/// seconds, so there is no refresh control to wire.
pub fn list(nav: Nav) -> Screen {
    let live = live();

    nav_page()
        .title("Agents")
        .header(
            view()
                .h(px(48.0))
                .flex_row()
                .items_center()
                .justify_between()
                .border_b_1()
                .border_color(rgb(theme::LINE))
                .bg(rgb(theme::SURFACE))
                .px_4()
                .child(
                    text("Agents")
                        .text_size(px(17.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(theme::TEXT)),
                )
                .child(open_inbox(nav)),
        )
        .child(
            view()
                .flex_1()
                .flex_col()
                .bg(rgb(theme::GRAPHITE))
                .child(link_line(move || live.link.get()))
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .px_4()
                        .py_2()
                        .child(
                            text(move || {
                                let count = live
                                    .snapshot
                                    .with(|snap| snap.as_ref().map_or(0, |snap| snap.agents.len()));
                                match count {
                                    1 => "1 agent".to_owned(),
                                    _ => format!("{count} agents"),
                                }
                            })
                            .text_size(px(12.0))
                            .text_color(rgb(theme::DIM))
                            .flex_1(),
                        )
                        .child(open_inbox(nav)),
                )
                .child(
                    when(
                        move || live.error.get().is_some(),
                        move || {
                            error_line(move || {
                                live.error
                                    .get()
                                    .unwrap_or_default()
                                    .into()
                            })
                        },
                    ),
                )
                .child(
                    virtual_list(
                        move || {
                            live.snapshot
                                .with(|snap| snap.as_ref().map_or(0, |snap| snap.agents.len()))
                        },
                        move |at| {
                            view()
                                .h(px(56.0))
                                .flex_row()
                                .items_center()
                                .gap_3()
                                .px_4()
                                .on_press(move || {
                                    let id = live
                                        .snapshot
                                        .with(|snap| {
                                            snap.as_ref()
                                                .and_then(|snap| snap.agents.get(at.index()).cloned())
                                                .map(|agent| agent.id.clone())
                                        });
                                    if let Some(id) = id {
                                        nav.push(Route::Agent(id));
                                    }
                                })
                                .child(
                                    text(move || {
                                        live.snapshot
                                            .with(|snap| {
                                                snap.as_ref()
                                                    .and_then(|snap| snap.agents.get(at.index()).cloned())
                                                    .map(|agent| agent.label().to_owned())
                                            })
                                            .unwrap_or_default()
                                    })
                                    .text_size(px(16.0))
                                    .text_color(rgb(theme::TEXT))
                                    .number_of_lines(1),
                                )
                                .child(status_pill(move || {
                                    live.snapshot
                                        .with(|snap| {
                                            snap.as_ref()
                                                .and_then(|snap| snap.agents.get(at.index()).cloned())
                                                .map(|agent| agent.status.clone())
                                        })
                                        .unwrap_or_default()
                                        .into()
                                }))
                        },
                    )
                    .estimated_item_size(px(56.0))
                    .keys(move || {
                        live.snapshot
                            .with(|snap| {
                                snap.as_ref()
                                    .map(|snap| {
                                        snap.agents.iter().map(|agent| agent.id.clone()).collect::<Vec<_>>()
                                    })
                                    .unwrap_or_default()
                            })
                    })
                    .separator(move || view().h(px(1.0)).mx_4().bg(rgb(theme::LINE)))
                    .empty(
                        view().p_8().items_center().child(
                            text(move || match live.link.get() {
                                crate::net::live::Link::Connecting => "Connecting…".to_owned(),
                                crate::net::live::Link::Open => "No agents".to_owned(),
                                _ => String::new(),
                            })
                            .text_size(px(15.0))
                            .text_color(rgb(theme::DIM)),
                        ),
                    )
                    .flex_1(),
                ),
        )
}
