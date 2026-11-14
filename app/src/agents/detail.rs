//! One agent: its status, herdr's recent output, and the pane frames the
//! live link watches for it.

use nsh_base::nav_page;
use nsh_core::prelude::*;

use crate::agents::{error_line, error_note, page_header, status_pill};
use crate::net::live::{live, Live, PaneFeed};
use crate::net::types::AgentDetail;
use crate::theme;
use crate::Nav;

/// One agent, pushed with `Route::Agent(id)`. The id the bridge resolves
/// (`w1:p2`, a name) is the page's title and the route's key.
pub fn detail(nav: Nav, id: String) -> Screen {
    let live = live();

    // herdr's recent output for the agent, fetched once on mount.
    let fetch_id = id.clone();
    let output = resource(
        move || Some(()),
        move |(), _| {
            let id = fetch_id.clone();
            async move {
                let bridge = crate::bridge().map_err(|err| err.to_string())?;
                bridge.agent(&id).await.map_err(|err| err.to_string())
            }
        },
    );

    // The agent's pane: watched once the snapshot names it, and left when
    // the page does. The guard keeps the 3 s snapshot refresh from
    // watch/unwatch churn.
    let watch_id = id.clone();
    let watched = signal(None::<String>);
    effect(move || {
        if watched.get_untracked().is_some() {
            return;
        }
        let pane = live.snapshot.with(|snap| {
            snap.as_ref()
                .and_then(|snap| snap.agent(&watch_id))
                .map(|agent| agent.pane_id.clone())
        });
        if let Some(pane) = pane {
            live.watch(&pane);
            watched.set(Some(pane));
        }
    });
    cleanup(move || {
        if let Some(pane) = watched.get_untracked() {
            live.unwatch(&pane);
        }
    });

    let body_id = id.clone();
    nav_page()
        .title(id)
        .header(page_header("Agent", nav))
        .child(
            view()
                .flex_1()
                .flex_col()
                .bg(rgb(theme::GRAPHITE))
                .child(link_bar(live))
                .child(when(
                    move || live.error.get().is_some(),
                    move || error_line(move || live.error.get().unwrap_or_default().into()),
                ))
                .child(when(
                    move || output.error().is_some(),
                    move || {
                        error_note(
                            move || {
                                output
                                    .error()
                                    .map(|err| err.to_owned())
                                    .unwrap_or_default()
                                    .into()
                            },
                            move || output.refetch(),
                        )
                    },
                ))
                .child(when(
                    move || live.snapshot.get().is_some(),
                    move || detail_body(live, body_id.clone().into(), output),
                ))
                .child(when(
                    move || live.snapshot.get().is_none() && live.error.get().is_none(),
                    || {
                        view().flex_1().items_center().justify_center().child(
                            text("Loading…")
                                .text_size(px(15.0))
                                .text_color(rgb(theme::DIM)),
                        )
                    },
                )),
        )
}

/// The link line, for the top of a pushed page's content.
fn link_bar(live: Live) -> impl IntoElement {
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
                .bg(move || match live.link.get() {
                    crate::net::live::Link::Open => rgb(theme::TEAL),
                    crate::net::live::Link::Connecting => rgb(theme::DIM),
                    _ => rgb(theme::GOLD),
                }),
        )
        .child(
            text(move || match live.link.get() {
                crate::net::live::Link::Open => "Live".to_owned(),
                crate::net::live::Link::Connecting => "Connecting…".to_owned(),
                crate::net::live::Link::Closed(why) => format!("Reconnecting: {why}"),
                crate::net::live::Link::Unconfigured(why) => format!("No bridge address: {why}"),
            })
            .text_size(px(12.0))
            .text_color(move || match live.link.get() {
                crate::net::live::Link::Open => rgb(theme::TEXT),
                crate::net::live::Link::Connecting => rgb(theme::DIM),
                _ => rgb(theme::GOLD),
            })
            .number_of_lines(1),
        )
}

/// Status and identity from the snapshot, then the pane feed and the
/// fetched output. One owned id per reactive closure: `SharedString` is an
/// `Arc`, so the clones are cheap.
fn detail_body(
    live: Live,
    id: SharedString,
    output: Resource<AgentDetail, String>,
) -> impl IntoElement {
    let id_label = id.clone();
    let id_status = id.clone();
    let id_meta = id.clone();
    let id_feed = id.clone();
    let id_refused = id.clone();
    let id_refused_text = id.clone();
    let id_output = id.clone();

    view()
        .flex_1()
        .flex_col()
        .child(
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
                                live.snapshot
                                    .with(|snap| {
                                        snap.as_ref()
                                            .and_then(|snap| snap.agent(&id_label).cloned())
                                            .map(|agent| agent.label().to_owned())
                                    })
                                    .unwrap_or_else(|| id_label.to_string())
                            })
                            .text_size(px(20.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(theme::TEXT))
                            .number_of_lines(1),
                        )
                        .child(status_pill(move || {
                            live.snapshot
                                .with(|snap| {
                                    snap.as_ref()
                                        .and_then(|snap| snap.agent(&id_status).cloned())
                                        .map(|agent| agent.status.clone())
                                })
                                .unwrap_or_default()
                                .into()
                        })),
                )
                .child(
                    text(move || {
                        match live.snapshot.with(|snap| {
                            snap.as_ref()
                                .and_then(|snap| snap.agent(&id_meta))
                                .map(|agent| {
                                    agent.agent.clone().unwrap_or_else(|| "agent".to_owned())
                                })
                        }) {
                            Some(harness) => harness,
                            None => "Not in the snapshot.".to_owned(),
                        }
                    })
                    .text_size(px(13.0))
                    .text_color(rgb(theme::DIM)),
                ),
        )
        .child(
            // The watched pane's frames, and what the watch sent first.
            view().p_4().pb_2().flex_col().gap_2().child(
                text(move || match feed(live, &id_feed) {
                    Some(feed) => format!(
                        "frames {} · seq {} · out of order {}",
                        feed.seq.frames,
                        feed.seq
                            .last()
                            .map(|seq| seq.to_string())
                            .unwrap_or_else(|| "-".to_owned()),
                        feed.seq.out_of_order
                    ),
                    None => "Not watching the pane yet.".to_owned(),
                })
                .text_size(px(12.0))
                .text_color(rgb(theme::DIM)),
            ),
        )
        .child(when(
            move || {
                feed(live, &id_refused)
                    .and_then(|feed| feed.refused.clone())
                    .is_some()
            },
            move || {
                let id = id_refused_text.clone();
                text(move || {
                    format!(
                        "The bridge refused: {}",
                        feed(live, &id)
                            .and_then(|feed| feed.refused.clone())
                            .unwrap_or_default()
                    )
                })
                .text_size(px(12.0))
                .text_color(rgb(theme::GOLD))
            },
        ))
        .child(
            // herdr's recent lines for this pane: the watch's scrollback
            // first, the fetched output behind it.
            view()
                .flex_1()
                .flex_col()
                .gap_2()
                .px_4()
                .pb_4()
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
                                let feed = feed(live, &id_output);
                                if let Some(text) =
                                    feed.as_ref().and_then(|feed| feed.scrollback.clone())
                                {
                                    return text;
                                }
                                output
                                    .get()
                                    .and_then(|detail| detail.output_text().map(str::to_owned))
                                    .unwrap_or_else(|| "(no recent output)".to_owned())
                            })
                            .text_size(px(13.0))
                            .text_color(rgb(theme::TEXT)),
                        ),
                ),
        )
}

/// The pane feed for this agent, if the pane is known and watched. Reads
/// through the signals, so it tracks the caller's reactive scope.
fn feed(live: Live, id: &str) -> Option<PaneFeed> {
    let pane = live.snapshot.with(|snap| {
        snap.as_ref()
            .and_then(|snap| snap.agent(id))
            .map(|agent| agent.pane_id.clone())
    })?;
    live.panes.with(|panes| panes.get(&pane).cloned())
}
