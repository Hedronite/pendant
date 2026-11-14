//! One agent: its status, herdr's recent output, the pane frames the live
//! link watches for it, and the writes it takes.

use std::cell::RefCell;
use std::future::Future;
use std::rc::Rc;

use nsh_base::alert_dialog;
use nsh_base::nav_page;
use nsh_core::element::input;
use nsh_core::prelude::*;

use crate::agents::{
    error_line, error_note, page_header, status_pill, unpaired_banner, write_button, write_note,
    ButtonKind,
};
use crate::net::live::{live, Live, PaneFeed};
use crate::net::pair::{PairState, WorktreeRequest, WriteError};
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
    //
    // The watched id lives in an Rc slot, not a signal: the page owner's
    // signals are disposed before its cleanups run, and a cleanup that
    // reads a disposed signal fails without unwatching (a Back then keeps
    // the pane's frames flowing).
    let watch_id = id.clone();
    let watched = Rc::new(RefCell::new(None::<String>));
    let effect_watched = Rc::clone(&watched);
    effect(move || {
        if effect_watched.borrow().is_some() {
            return;
        }
        let pane = live.snapshot.with(|snap| {
            snap.as_ref()
                .and_then(|snap| snap.agent(&watch_id))
                .map(|agent| agent.pane_id.clone())
        });
        if let Some(pane) = pane {
            live.watch(&pane);
            *effect_watched.borrow_mut() = Some(pane);
        }
    });
    let cleanup_watched = watched;
    cleanup(move || {
        if let Some(pane) = cleanup_watched.borrow_mut().take() {
            live.unwatch(&pane);
        }
    });

    // The write controls. One `busy` at a time: a second biometric prompt
    // does not stack on the first.
    let busy = signal(false);
    let write_error = signal(None::<WriteError>);
    let draft = signal(String::new());
    let made = signal(None::<String>);

    let paired = move || matches!(live.pairing.get(), PairState::Paired { .. });
    let enabled = move || !busy.get() && paired();

    let prompt_id = id.clone();
    let on_prompt = move || {
        let text = draft.get().trim().to_owned();
        if text.is_empty() {
            return;
        }
        let live = live;
        let id = prompt_id.clone();
        spawn(async move {
            busy.set(true);
            error_set(&write_error, None);
            let result = with_grant(live, || live.prompt(&id, &text)).await;
            busy.set(false);
            match result {
                Ok(()) => draft.set(String::new()),
                Err(err) => error_set(&write_error, Some(err)),
            }
        });
    };

    let approve_id = id.clone();
    let on_approve = move || {
        let live = live;
        let id = approve_id.clone();
        spawn(async move {
            busy.set(true);
            error_set(&write_error, None);
            let result = with_grant(live, || live.approve(&id)).await;
            busy.set(false);
            if let Err(err) = result {
                error_set(&write_error, Some(err));
            }
        });
    };

    let deny_id = id.clone();
    let on_deny = move || {
        let answer = alert_dialog()
            .title("Deny the agent?")
            .description("The agent's waiting request is refused.")
            .cancel("cancel", "Cancel")
            .destructive("deny", "Deny")
            .show();
        let live = live;
        let id = deny_id.clone();
        spawn(async move {
            if answer
                .await
                .is_ok_and(|r| r.action.as_deref() == Some("deny"))
            {
                busy.set(true);
                error_set(&write_error, None);
                let result = with_grant(live, || live.deny(&id)).await;
                busy.set(false);
                if let Err(err) = result {
                    error_set(&write_error, Some(err));
                }
            }
        });
    };

    let worktree_id = id.clone();
    let on_worktree = move || {
        let live = live;
        let id = worktree_id.clone();
        spawn(async move {
            busy.set(true);
            error_set(&write_error, None);
            let request = live.snapshot.with_untracked(|snap| {
                snap.as_ref()
                    .and_then(|snap| snap.agent(&id))
                    .map(|agent| WorktreeRequest {
                        workspace_id: Some(agent.workspace_id.clone()),
                        ..Default::default()
                    })
            });
            let Some(request) = request else {
                busy.set(false);
                return;
            };
            let result = with_grant(live, || live.worktree_create(&request)).await;
            busy.set(false);
            match result {
                Ok(_) => made.set(Some("Worktree created.".to_owned())),
                Err(err) => error_set(&write_error, Some(err)),
            }
        });
    };

    let body_id = id.clone();
    let write_id = id.clone();
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
                    move || matches!(live.pairing.get(), PairState::Unpaired { .. }),
                    move || unpaired_banner(nav),
                ))
                .child(when(
                    move || live.snapshot.get().is_some(),
                    move || {
                        // The branch runs again on every flip, so each run
                        // gets its own copies of the handlers.
                        let body_id = body_id.clone();
                        let write_id = write_id.clone();
                        let enabled = enabled.clone();
                        let on_prompt = on_prompt.clone();
                        let on_approve = on_approve.clone();
                        let on_deny = on_deny.clone();
                        let on_worktree = on_worktree.clone();
                        view()
                            .flex_1()
                            .flex_col()
                            .child(detail_body(live, body_id.into(), output))
                            .child(write_section(
                                live,
                                draft,
                                made,
                                write_error,
                                enabled,
                                on_prompt,
                                on_worktree,
                            ))
                            .child(when(
                                move || blocked(live, &write_id) && paired(),
                                move || {
                                    decision_bar(
                                        enabled.clone(),
                                        on_approve.clone(),
                                        on_deny.clone(),
                                    )
                                },
                            ))
                    },
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

/// Writes run with a grant open: with none, the grant's own biometric runs
/// first, so a write does not dead-end on `GrantRequired`. The write's
/// future is made after the grant opens, so its borrows stay in scope.
async fn with_grant<T, F: Future<Output = Result<T, WriteError>>>(
    live: Live,
    run: impl FnOnce() -> F,
) -> Result<T, WriteError> {
    if !live.grant_open() {
        live.open_grant().await?;
    }
    run().await
}

/// Sets the write error from a task.
fn error_set(error: &Signal<Option<WriteError>>, value: Option<WriteError>) {
    error.set(value);
}

/// Whether the agent is waiting on the user right now.
fn blocked(live: Live, id: &str) -> bool {
    live.snapshot
        .with(|snap| {
            snap.as_ref()
                .and_then(|snap| snap.agent(id))
                .map(|agent| agent.is_blocked())
        })
        .unwrap_or(false)
}

/// The write controls: the grant state, the prompt input, the worktree
/// action, and the last write's error.
fn write_section(
    live: Live,
    draft: Signal<String>,
    made: Signal<Option<String>>,
    error: Signal<Option<WriteError>>,
    enabled: impl Fn() -> bool + Clone + 'static,
    on_prompt: impl Fn() + Clone + 'static,
    on_worktree: impl Fn() + Clone + 'static,
) -> impl IntoElement {
    let enabled_send = enabled.clone();
    let enabled_wt = enabled;
    view()
        .flex_col()
        .gap_3()
        .p_4()
        .pt_3()
        .child(
            text(move || {
                if live.grant_open() {
                    "grant open".to_owned()
                } else {
                    "grant closed — the next write opens one".to_owned()
                }
            })
            .text_size(px(12.0))
            .text_color(rgb(theme::DIM)),
        )
        .child(when(
            move || made.get().is_some(),
            move || {
                text(move || made.get().unwrap_or_default())
                    .text_size(px(13.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(theme::TEAL))
            },
        ))
        .child(
            input()
                .value(move || draft.get())
                .placeholder("Prompt the agent…")
                .multiline(true)
                .on_change(move |text: &str| draft.set(text.to_owned()))
                .text_size(px(14.0))
                .text_color(rgb(theme::TEXT)),
        )
        .child(
            view()
                .flex_row()
                .gap_3()
                .child(view().flex_1().child(write_button(
                    "Send prompt",
                    move || enabled_send() && !draft.get().trim().is_empty(),
                    on_prompt,
                    ButtonKind::Primary,
                )))
                .child(view().flex_1().child(write_button(
                    "New worktree",
                    enabled_wt,
                    on_worktree,
                    ButtonKind::Quiet,
                ))),
        )
        .child(write_note(move || error.get()))
}

/// The bar pinned under the output while the agent is blocked.
fn decision_bar(
    enabled: impl Fn() -> bool + Clone + 'static,
    on_approve: impl Fn() + Clone + 'static,
    on_deny: impl Fn() + Clone + 'static,
) -> impl IntoElement {
    let enabled_deny = enabled.clone();
    view()
        .flex_row()
        .gap_3()
        .p_4()
        .pt_3()
        .border_t_1()
        .border_color(rgb(theme::LINE))
        .bg(rgb(theme::SURFACE))
        .child(view().flex_1().child(write_button(
            "Approve",
            enabled,
            on_approve,
            ButtonKind::Primary,
        )))
        .child(view().flex_1().child(write_button(
            "Deny",
            enabled_deny,
            on_deny,
            ButtonKind::Quiet,
        )))
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
