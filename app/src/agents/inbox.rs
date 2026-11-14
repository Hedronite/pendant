//! The inbox: agents that are blocked or done, newest first.

use nsh_base::nav_page;
use nsh_core::prelude::*;

use crate::agents::{error_note, event_label, page_header, status_pill};
use crate::theme;
use crate::{Nav, Route};

/// The inbox, pushed with `Route::Inbox`. Today it reads `GET /v1/events`
/// once on mount, with pull to refresh; live WebSocket updates wait on a PM
/// ruling (SPEC-shell G1).
pub fn inbox(nav: Nav) -> Screen {
    let inbox = resource(
        move || Some(()),
        move |(), _| async move {
            let bridge = crate::bridge().map_err(|err| err.to_string())?;
            bridge.inbox().await.map_err(|err| err.to_string())
        },
    );

    nav_page()
        .title("Inbox")
        .header(page_header("Inbox", nav))
        .child(
            view()
                .flex_1()
                .bg(rgb(theme::GRAPHITE))
                .child(
                    when(
                        move || inbox.error().is_some(),
                        move || {
                            error_note(
                                move || {
                                    inbox
                                        .error()
                                        .map(|err| err.to_owned())
                                        .unwrap_or_default()
                                        .into()
                                },
                                move || inbox.refetch(),
                            )
                        },
                    ),
                )
                .child(
                    virtual_list(
                        move || inbox.get().map_or(0, |events| events.items.len()),
                        move |at| {
                            view()
                                .min_h(px(56.0))
                                .flex_row()
                                .items_center()
                                .gap_3()
                                .px_4()
                                .py_2()
                                .on_press(move || {
                                    let id = inbox
                                        .get()
                                        .and_then(|events| events.items.get(at.index()).cloned())
                                        .map(|item| item.id.clone());
                                    if let Some(id) = id {
                                        nav.push(Route::Agent(id));
                                    }
                                })
                                .child(
                                    // The agent the event is about: its
                                    // title, else its name, else its id.
                                    text(move || {
                                        inbox
                                            .get()
                                            .and_then(|events| events.items.get(at.index()).cloned())
                                            .map(|item| event_label(&item))
                                            .unwrap_or_default()
                                    })
                                    .text_size(px(16.0))
                                    .text_color(rgb(theme::TEXT))
                                    .number_of_lines(1)
                                    .flex_1(),
                                )
                                .child(status_pill(move || {
                                    inbox
                                        .get()
                                        .and_then(|events| events.items.get(at.index()).cloned())
                                        .map(|item| item.kind.clone())
                                        .unwrap_or_default()
                                })),
                        },
                    )
                    .estimated_item_size(px(64.0))
                    .keys(move || {
                        inbox
                            .get()
                            .map(|events| {
                                events
                                    .items
                                    .iter()
                                    .map(|item| item.id.clone())
                                    .collect::<Vec<_>>()
                            })
                            .unwrap_or_default()
                    })
                    .separator(move || view().h(px(1.0)).mx_4().bg(rgb(theme::LINE)))
                    .on_refresh(move || inbox.refetch())
                    .refreshing(move || inbox.loading())
                    .empty(|| {
                        view().p_8().items_center().child(
                            text("Nothing needs you")
                                .text_size(px(15.0))
                                .text_color(rgb(theme::DIM)),
                        )
                    })
                    .flex_1(),
                ),
        )
}
