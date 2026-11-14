//! One agent: its status, and herdr's recent output for it.

use nsh_base::nav_page;
use nsh_core::prelude::*;

use crate::agents::{error_note, page_header, status_pill};
use crate::theme;
use crate::Nav;

/// One agent, pushed with `Route::Agent(id)`. The id the bridge resolves
/// (`w1:p2`, a name) is the page's title and the route's key.
pub fn detail(nav: Nav, id: String) -> Screen {
    let load = resource(
        move || Some(()),
        move |(), _| {
            let id = id.clone();
            async move {
                let bridge = crate::bridge().map_err(|err| err.to_string())?;
                bridge.agent(&id).await.map_err(|err| err.to_string())
            }
        },
    );

    nav_page()
        .title(id)
        .header(page_header("Agent", nav))
        .child(
            view()
                .flex_1()
                .bg(rgb(theme::GRAPHITE))
                .child(
                    when(
                        move || load.error().is_some(),
                        move || {
                            error_note(
                                move || {
                                    load.error()
                                        .map(|err| err.to_owned())
                                        .unwrap_or_default()
                                        .into()
                                },
                                move || load.refetch(),
                            )
                        },
                    ),
                )
                .child(
                    when(move || load.get().is_some(), move || {
                        view().flex_1().flex_col().child(
                            // Status and identity.
                            view()
                                .flex_col()
                                .gap_2()
                                .p_4()
                                .border_b_1()
                                .border_color(rgb(theme::LINE))
                                .child(
                                    view()
                                        .flex_row()
                                        .items_center()
                                        .gap_3()
                                        .child(
                                            text(move || {
                                                load.get()
                                                    .and_then(|detail| detail.agent.as_ref())
                                                    .map(|agent| agent.label().to_owned())
                                                    .unwrap_or_default()
                                            })
                                            .text_size(px(20.0))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(theme::TEXT))
                                            .number_of_lines(1),
                                        )
                                        .child(status_pill(move || {
                                            load.get()
                                                .and_then(|detail| detail.agent.as_ref())
                                                .map(|agent| agent.status.clone())
                                                .unwrap_or_default()
                                        })),
                                )
                                .child(
                                    text(move || {
                                        match load.get() {
                                            Some(detail) => match detail.agent.as_ref() {
                                                Some(agent) => format!(
                                                    "{} · workspace {}",
                                                    agent.agent.as_deref().unwrap_or("agent"),
                                                    agent.workspace_id
                                                ),
                                                None if detail.blocked => {
                                                    "Blocked, but no longer in the snapshot.".to_owned()
                                                }
                                                None => "Not in the snapshot.".to_owned(),
                                            },
                                            None => String::new(),
                                        }
                                    })
                                    .text_size(px(13.0))
                                    .text_color(rgb(theme::DIM)),
                                ),
                        )
                        .child(
                            // herdr's recent lines for this pane.
                            view()
                                .flex_1()
                                .flex_col()
                                .gap_2()
                                .p_4()
                                .child(
                                    text("Recent output")
                                        .text_size(px(13.0))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(rgb(theme::DIM)),
                                )
                                .child(
                                    scroll()
                                        .flex_1()
                                        .bg(rgb(theme::SURFACE))
                                        .rounded(px(10.0))
                                        .p_3()
                                        .child(
                                            text(move || {
                                                load.get()
                                                    .and_then(|detail| {
                                                        detail.output_text().map(str::to_owned)
                                                    })
                                                    .unwrap_or_else(|| {
                                                        "(no recent output)".to_owned()
                                                    })
                                            })
                                            .text_size(px(13.0))
                                            .text_color(rgb(theme::TEXT)),
                                        ),
                                ),
                        )
                    })
                    .otherwise(|| {
                        when(
                            move || load.error().is_none(),
                            || {
                                view().flex_1().items_center().justify_center().child(
                                    text("Loading…")
                                        .text_size(px(15.0))
                                        .text_color(rgb(theme::DIM)),
                                )
                            },
                        )
                    }),
                ),
        )
}
