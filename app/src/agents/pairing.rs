//! Pairing: paste the payload from the bridge host, or arrive with it from
//! a `pendant://pair` link.

use nsh_base::nav_page;
use nsh_core::element::input;
use nsh_core::prelude::*;

use crate::agents::{page_header, write_button, write_note, ButtonKind};
use crate::net::live::{live, Live};
use crate::net::pair::{PairState, Payload, WriteError};
use crate::theme;
use crate::Nav;

/// The pairing page. A payload carried by a `pendant://` link pairs at
/// once; otherwise the payload is pasted or typed, then paired. Every path
/// spends one biometric — the pair signature.
pub fn pairing(nav: Nav, payload: Option<Payload>) -> Screen {
    let live = live();
    let draft = signal(String::new());
    let error = signal(None::<WriteError>);

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
            spawn(async move {
                match live.pair_with(&payload).await {
                    Ok(()) => nav.back(),
                    Err(err) => error.set(Some(err)),
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
                    move || pair_controls(live, draft, error),
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
fn pair_controls(
    live: Live,
    draft: Signal<String>,
    error: Signal<Option<WriteError>>,
) -> impl IntoElement {
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
                        spawn(async move {
                            error.set(None);
                            if let Err(err) = live.pair(&code).await {
                                error.set(Some(err));
                            }
                        });
                    },
                    ButtonKind::Primary,
                )),
        )
        .child(write_note(move || error.get()))
}
