//! The agent list: the app's bottom page.

use nsh_base::nav_page;
use nsh_core::prelude::*;

use crate::agents::{error_note, status_pill};
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

/// The agent list, the app's bottom page.
pub fn list(nav: Nav) -> Screen {
    let bridge = crate::bridge().map_err(|err| err.to_string());
    let snapshot = resource(
        move || Some(()),
        move |(), _| {
            let bridge = bridge.clone();
            async move { bridge?.snapshot().await.map_err(|err| err.to_string()) }
        },
    );

    nav_page()
        .title("Agents")
        .header_right(open_inbox(nav))
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
                .bg(rgb(theme::GRAPHITE))
                .child(
                    when(
                        move || snapshot.error().is_some(),
                        move || {
                            error_note(
                                move || {
                                    snapshot
                                        .error()
                                        .map(|err| err.to_owned())
                                        .unwrap_or_default()
                                        .into()
                                },
                                move || snapshot.refetch(),
                            )
                        },
                    ),
                )
                .child(
                    virtual_list(
                        move || snapshot.get().map_or(0, |snap| snap.agents.len()),
                        move |at| {
                            view()
                                .h(px(56.0))
                                .flex_row()
                                .items_center()
                                .gap_3()
                                .px_4()
                                .on_press(move || {
                                    let id = snapshot
                                        .get()
                                        .and_then(|snap| snap.agents.get(at.index()).cloned())
                                        .map(|agent| agent.id.clone());
                                    if let Some(id) = id {
                                        nav.push(Route::Agent(id));
                                    }
                                })
                                .child(
                                    text(move || {
                                        snapshot
                                            .get()
                                            .and_then(|snap| snap.agents.get(at.index()).cloned())
                                            .map(|agent| agent.label().to_owned())
                                            .unwrap_or_default()
                                    })
                                    .text_size(px(16.0))
                                    .text_color(rgb(theme::TEXT))
                                    .number_of_lines(1),
                                )
                                .child(status_pill(move || {
                                    snapshot
                                        .get()
                                        .and_then(|snap| snap.agents.get(at.index()).cloned())
                                        .map(|agent| agent.status.clone())
                                        .unwrap_or_default()
                                })),
                        },
                    )
                    .estimated_item_size(px(56.0))
                    .keys(move || {
                        snapshot
                            .get()
                            .map(|snap| {
                                snap.agents
                                    .iter()
                                    .map(|agent| agent.id.clone())
                                    .collect::<Vec<_>>()
                            })
                            .unwrap_or_default()
                    })
                    .separator(move || view().h(px(1.0)).mx_4().bg(rgb(theme::LINE)))
                    .on_refresh(move || snapshot.refetch())
                    .refreshing(move || snapshot.loading())
                    .empty(|| {
                        view().p_8().items_center().child(
                            text("No agents")
                                .text_size(px(15.0))
                                .text_color(rgb(theme::DIM)),
                        )
                    })
                    .flex_1(),
                ),
        )
}
