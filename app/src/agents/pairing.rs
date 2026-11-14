//! Pairing: paste the payload from the bridge host, or arrive with it from
//! a `pendant://pair` link.

use nsh_base::nav_page;
use nsh_core::element::{create_root, input};
use nsh_core::prelude::*;
use std::cell::RefCell;
use std::future::Future;
use std::rc::Rc;

use crate::agents::{page_header, write_button, ButtonKind};
use crate::net::live::{live, Live};
use crate::net::pair::{PairState, Payload};
use crate::theme;
use crate::Nav;

/// A detached task kept alive: its root owner and its handle, released
/// by the task itself when it ends.
type DetachedTask = Option<(Owner, Spawned<()>)>;

/// Runs a pair task in a detached root: the pairing page owns its signals,
/// not the task, so the page going away does not cancel a pair in flight
/// (a dismissal used to cancel it 71 ms in, before the POST). The page
/// reads [`PairState`] for what happened; the task touches nothing the
/// page owns.
fn detached_pair(future: impl Future<Output = ()> + 'static) {
    // Dropping the spawned handle cancels the task before its first
    // poll, so the root owner and the handle live in a slot the task
    // empties as its last step: the pair is never cancelled this way.
    let slot: Rc<RefCell<DetachedTask>> = Rc::new(RefCell::new(None));
    let keep = Rc::clone(&slot);
    let (owner, task) = create_root(move || {
        spawn(async move {
            future.await;
            keep.borrow_mut().take();
        })
    });
    *slot.borrow_mut() = Some((owner, task));
}

/// The pairing page. A payload carried by a `pendant://` link pairs at
/// once; otherwise the payload is pasted or typed, then paired. Every path
/// spends one biometric — the pair signature.
pub fn pairing(nav: Nav, payload: Option<Payload>) -> Screen {
    let live = live();
    let draft = signal(String::new());

    // The link's payload: pair once on arrival, then leave.
    let carried = payload;
    let started = signal(false);
    effect(move || {
        if started.get_untracked() {
            return;
        }
        if let Some(payload) = carried.clone() {
            started.set(true);
            let live = live;
            detached_pair(async move {
                if live.pair_with(&payload).await.is_ok() {
                    nav.back();
                }
            });
        }
    });

    nav_page()
        .title("Pair")
        .header(page_header("Pair", nav))
        .child(
            view()
                .flex_1()
                .flex_col()
                .bg(rgb(theme::GRAPHITE))
                .child(
                    text("Pairing connects this phone to the bridge. The payload comes from the bridge host's pairing page: copy it there, then paste it here.")
                        .text_size(px(13.0))
                        .text_color(rgb(theme::DIM))
                        .p_4(),
                )
                .child(when(
                    move || live.pairing.get() == PairState::Pairing,
                    || {
                        view().flex_row().items_center().gap_2().px_4().pb_2().child(
                            text("Pairing — approve the biometric prompt…")
                                .text_size(px(13.0))
                                .text_color(rgb(theme::DIM)),
                        )
                    },
                ))
                .child(when(
                    move || matches!(live.pairing.get(), PairState::Paired { .. }),
                    move || paired_card(live, nav),
                ))
                .child(when(
                    move || !matches!(live.pairing.get(), PairState::Paired { .. }),
                    move || pair_controls(live, draft),
                )),
        )
}

/// What a paired device sees on the pairing page.
fn paired_card(live: Live, nav: Nav) -> impl IntoElement {
    view()
        .flex_col()
        .gap_3()
        .p_4()
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .child(view().size(px(10.0)).rounded(px(5.0)).bg(rgb(theme::TEAL)))
                .child(
                    text("Paired")
                        .text_size(px(17.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(theme::TEXT)),
                ),
        )
        .child(
            text(move || match live.pairing.get() {
                PairState::Paired { device_id } => device_id,
                _ => String::new(),
            })
            .text_size(px(12.0))
            .text_color(rgb(theme::DIM))
            .number_of_lines(1),
        )
        .child(
            text("Done")
                .text_size(px(15.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(theme::TEAL))
                .on_press(move || nav.back())
                .role(AccessibilityRole::Button),
        )
}

/// The payload input, the paste and pair buttons, and what went wrong.
fn pair_controls(live: Live, draft: Signal<String>) -> impl IntoElement {
    let enabled = move || {
        live.pairing.get() != PairState::Pairing
            && !matches!(live.pairing.get(), PairState::Paired { .. })
    };
    view()
        .flex_col()
        .gap_3()
        .p_4()
        .child(
            input()
                .test_id("pair-input")
                .value(move || draft.get())
                .placeholder("pendant-pair:v1:…")
                .auto_capitalize(AutoCapitalize::None)
                .auto_correct(false)
                .on_change(move |text: &str| draft.set(text.to_owned()))
                .text_size(px(13.0))
                .text_color(rgb(theme::TEXT)),
        )
        .child(
            view()
                .flex_row()
                .gap_3()
                .child(write_button(
                    "paste",
                    "Paste from clipboard",
                    enabled,
                    move || {
                        spawn(async move {
                            let text = clipboard_string().await;
                            if !text.trim().is_empty() {
                                draft.set(text);
                            }
                        });
                    },
                    ButtonKind::Quiet,
                ))
                .child(write_button(
                    "pair-submit",
                    "Pair",
                    move || enabled() && !draft.get().trim().is_empty(),
                    move || {
                        let code = draft.get();
                        detached_pair(async move {
                            live.pair(&code).await.ok();
                        });
                    },
                    ButtonKind::Primary,
                )),
        )
        // A pair that did not happen: `pair_with` writes the reason into
        // the pairing state, which this page only reads.
        .child(when(
            move || matches!(live.pairing.get(), PairState::Unpaired { reason: Some(_) }),
            move || {
                text(move || match live.pairing.get() {
                    PairState::Unpaired {
                        reason: Some(reason),
                    } => reason,
                    _ => String::new(),
                })
                .test_id("write-error")
                .text_size(px(12.0))
                .text_color(rgb(theme::GOLD))
                .number_of_lines(3)
            },
        ))
}
