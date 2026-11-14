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
use crate::net::NetError;
use crate::theme;
use crate::Nav;

/// Which platform this build runs on, for made-up worktree names.
const WT_PLATFORM: &str = if cfg!(target_os = "android") {
    "android"
} else if cfg!(target_os = "ios") {
    "ios"
} else {
    "host"
};

/// The UTC `HHMMSS` stamp a default worktree branch carries, so two
/// creates a minute apart do not collide.
pub(crate) fn wt_stamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format!(
        "{:02}{:02}{:02}",
        (secs / 3600) % 24,
        (secs / 60) % 60,
        secs % 60
    )
}

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
    // does not stack on the first. Each write runs outside this page's
    // owner, so leaving the page does not cancel it; the page's state is
    // only written back while the page still lives.
    let page = Owner::current();
    let busy = signal(false);
    let write_error = signal(None::<WriteError>);
    let draft = signal(String::new());
    let wt_branch = signal(format!("pendant-wt-{WT_PLATFORM}-{}", wt_stamp()));
    let made = signal(None::<String>);
    let paired = move || matches!(live.pairing.get(), PairState::Paired { .. });
    let enabled = move || !busy.get() && paired();
    let prompt_id = id.clone();
    let send_prompt = move |text: String| {
        eprintln!("pendant: send_prompt fired");
        if text.is_empty() {
            eprintln!("pendant: send_prompt empty text, dropped");
            return;
        }
        let live = live;
        let id = prompt_id.clone();
        busy.set(true);
        error_set(&write_error, None);
        detached_write(
            page,
            async move { with_grant(live, || live.prompt(&id, &text)).await },
            move |result| {
                busy.set(false);
                match result {
                    Ok(()) => {
                        draft.set(String::new());
                        made.set(Some("Prompt sent.".to_owned()));
                    }
                    Err(err) => error_set(&write_error, Some(err)),
                }
            },
        );
    };
    let send_prompt = Rc::new(send_prompt);
    let prompt_send = Rc::clone(&send_prompt);
    let on_prompt = move || prompt_send(draft.get().trim().to_owned());
    let submit_send = Rc::clone(&send_prompt);
    let on_submit = move |text: &str| submit_send(text.trim().to_owned());
    let approve_id = id.clone();
    let on_approve = move || {
        eprintln!("pendant: approve fired");
        let live = live;
        let id = approve_id.clone();
        busy.set(true);
        error_set(&write_error, None);
        detached_write(
            page,
            async move { with_grant(live, || live.approve(&id)).await },
            move |result| {
                busy.set(false);
                if let Err(err) = result {
                    error_set(&write_error, Some(err));
                }
            },
        );
    };
    let deny_id = id.clone();
    let on_deny = move || {
        eprintln!("pendant: deny fired, asking to confirm");
        let answer = alert_dialog()
            .title("Deny the agent?")
            .description("The agent's waiting request is refused.")
            .cancel("cancel", "Cancel")
            .destructive("deny", "Deny")
            .show();
        let live = live;
        let id = deny_id.clone();
        spawn(async move {
            let chosen = answer
                .await
                .ok()
                .and_then(|r| r.action)
                .unwrap_or_else(|| "<dismissed>".into());
            eprintln!("pendant: deny dialog answered {chosen}");
            if chosen == "deny" {
                busy.set(true);
                error_set(&write_error, None);
                detached_write(
                    page,
                    async move { with_grant(live, || live.deny(&id)).await },
                    move |result| {
                        busy.set(false);
                        if let Err(err) = result {
                            error_set(&write_error, Some(err));
                        }
                    },
                );
            }
        });
    };
    let on_open_grant = move || {
        eprintln!("pendant: open_grant fired");
        let live = live;
        busy.set(true);
        error_set(&write_error, None);
        detached_write(
            page,
            async move { live.open_grant().await },
            move |result| {
                busy.set(false);
                if let Err(err) = result {
                    error_set(&write_error, Some(err));
                }
            },
        );
    };
    let worktree_id = id.clone();
    let on_worktree = move || {
        eprintln!("pendant: worktree fired");
        let live = live;
        let id = worktree_id.clone();
        busy.set(true);
        error_set(&write_error, None);
        detached_write(
            page,
            async move {
                let request = live.snapshot.with_untracked(|snap| {
                    snap.as_ref()
                        .and_then(|snap| snap.agent(&id))
                        .map(|agent| WorktreeRequest {
                            workspace_id: Some(agent.workspace_id.clone()),
                            branch: Some("pendant-g2-wt-sim".to_owned()),
                            ..Default::default()
                        })
                });
                let Some(request) = request else {
                    return Err(WriteError::Net(NetError::Transport(
                        "the agent is not in the snapshot".to_owned(),
                    )));
                };
                with_grant(live, || live.worktree_create(&request)).await
            },
            move |result| {
                busy.set(false);
                match result {
                    Ok(_) => made.set(Some("Worktree created.".to_owned())),
                    Err(err) => error_set(&write_error, Some(err)),
                }
            },
        );
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
                        // The branch runs again on every flip: non-Copy
                        // handlers are cloned per run, Copy ones just move.
                        let body_id = body_id.clone();
                        let write_id = write_id.clone();
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
                                wt_branch,
                                made,
                                write_error,
                                enabled,
                                on_prompt.clone(),
                                on_submit.clone(),
                                on_open_grant,
                                on_worktree,
                            ))
                            .child(when(
                                move || blocked(live, &write_id) && paired(),
                                move || decision_bar(enabled, on_approve.clone(), on_deny.clone()),
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

/// Runs a write outside the page's owner: leaving the page does not
/// cancel it (a pairing task died exactly that way). The `settle` step
/// touches the page's state only while the page still lives.
fn detached_write<T: 'static>(
    page: Owner,
    future: impl Future<Output = T> + 'static,
    settle: impl FnOnce(T) + 'static,
) {
    // Runs under the app root and keeps its task handle until it ends
    // (dropping an nsh task handle cancels the task); settles only while
    // the page is still there.
    crate::net::live::spawn_detached(async move {
        let out = future.await;
        if page.is_live() {
            settle(out);
        }
    });
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
#[allow(
    clippy::too_many_arguments,
    // One control per argument: the grant state, the note, the input,
    // and one closure per action.
)]
fn write_section(
    live: Live,
    draft: Signal<String>,
    wt_branch: Signal<String>,
    made: Signal<Option<String>>,
    error: Signal<Option<WriteError>>,
    enabled: impl Fn() -> bool + Copy + 'static,
    on_prompt: impl Fn() + Clone + 'static,
    on_submit: impl Fn(&str) + Clone + 'static,
    on_open_grant: impl Fn() + Copy + 'static,
    on_worktree: impl Fn() + Clone + 'static,
) -> impl IntoElement {
    let enabled_send = enabled;
    let enabled_wt = enabled;
    view()
        .flex_col()
        .gap_3()
        .p_4()
        .pt_3()
        .pb_safe_area_plus(px(16.0))
        .child(
            text(move || {
                if live.grant_open() {
                    "grant open".to_owned()
                } else {
                    "grant closed — the next write opens one".to_owned()
                }
            })
            .test_id("grant-line")
            .text_size(px(12.0))
            .text_color(rgb(theme::DIM)),
        )
        // Always one element: a when() here would re-create the input
        // below when the note appears or goes, and the re-created field
        // loses its on_change binding.
        .child(
            text(move || made.get().unwrap_or_default())
                .test_id("made-note")
                .text_size(px(13.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(theme::TEAL)),
        )
        .child(
            input()
                .test_id("prompt-input")
                .value(move || draft.get())
                .placeholder("Prompt the agent…")
                .multiline(true)
                .return_key_type(ReturnKeyType::Send)
                .submit_behavior(SubmitBehavior::Submit)
                .on_change(move |text: &str| draft.set(text.to_owned()))
                .on_submit(on_submit)
                .text_size(px(14.0))
                .text_color(rgb(theme::TEXT)),
        )
        .child(
            view()
                .flex_row()
                .gap_3()
                .child(view().flex_1().child(write_button(
                    "send-prompt",
                    "Send prompt",
                    move || enabled_send() && !draft.get().trim().is_empty(),
                    on_prompt,
                    ButtonKind::Primary,
                )))
                .child(view().flex_1().child(write_button(
                    "new-worktree",
                    "New worktree",
                    enabled_wt,
                    on_worktree,
                    ButtonKind::Quiet,
                ))),
        )
        .child(
            // Always one element, like the prompt field above: the branch
            // the create takes, prefilled with a unique default.
            input()
                .test_id("worktree-branch")
                .value(move || wt_branch.get())
                .placeholder("pendant-wt-…")
                .auto_capitalize(AutoCapitalize::None)
                .auto_correct(false)
                .on_change(move |text: &str| wt_branch.set(text.to_owned()))
                .text_size(px(13.0))
                .text_color(rgb(theme::TEXT)),
        )
        .child(
            // Always one row: a when() around this button would re-create
            // the input above when the grant state flips, and the
            // re-created field loses its on_change binding. The button
            // stays and simply reads as unavailable while a grant is open.
            view().child(write_button(
                "open-grant",
                "Open grant",
                move || enabled() && !live.grant_open(),
                on_open_grant,
                ButtonKind::Quiet,
            )),
        )
        .child(write_note(move || error.get()))
}

/// The bar pinned under the output while the agent is blocked.
fn decision_bar(
    enabled: impl Fn() -> bool + Copy + 'static,
    on_approve: impl Fn() + Clone + 'static,
    on_deny: impl Fn() + Clone + 'static,
) -> impl IntoElement {
    let enabled_deny = enabled;
    view()
        .test_id("decision-bar")
        .on_layout(|frame: &Bounds<Points>| {
            eprintln!(
                "pendant: decision-bar layout {}x{} at {},{}",
                frame.size.width.0, frame.size.height.0, frame.origin.x.0, frame.origin.y.0
            );
        })
        .flex_row()
        .gap_3()
        .p_4()
        .pt_3()
        .pb_safe_area_plus(px(16.0))
        .border_t_1()
        .border_color(rgb(theme::LINE))
        .bg(rgb(theme::SURFACE))
        .child(view().flex_1().child(write_button(
            "approve",
            "Approve",
            enabled,
            on_approve,
            ButtonKind::Primary,
        )))
        .child(view().flex_1().child(write_button(
            "deny",
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
