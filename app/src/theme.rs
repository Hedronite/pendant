//! Pendant's colours. Frontend owns this file: the three brand colours from
//! SPEC-shell, plus the supporting shades on Graphite the screens share.

/// Graphite, the background.
pub const GRAPHITE: u32 = 0x111513;
/// Teal, the accent.
pub const TEAL: u32 = 0x0F5C56;
/// Gold, for what needs the user: a blocked agent.
pub const GOLD: u32 = 0xC9A227;

/// A row or card, one step up from the background.
pub const SURFACE: u32 = 0x1B211E;
/// Separators and borders.
pub const LINE: u32 = 0x272F2B;
/// Primary text.
pub const TEXT: u32 = 0xE9EFEC;
/// Secondary text.
pub const DIM: u32 = 0x8FA098;

/// The pill a herdr status draws as: `(background, text)`.
///
/// `blocked` is the one loud state — Gold on Graphite is what a list row, a
/// detail page and an inbox item all share (SPEC-shell G1). `working` and
/// `done` carry the Teal accent; everything else stays quiet.
pub fn status_pill(status: &str) -> (u32, u32) {
    match status {
        "blocked" => (GOLD, GRAPHITE),
        "working" | "done" => (TEAL, TEXT),
        _ => (LINE, DIM),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocked_is_the_loud_pill() {
        assert_eq!(status_pill("blocked"), (GOLD, GRAPHITE));
    }

    #[test]
    fn active_statuses_carry_teal_and_the_rest_stay_quiet() {
        assert_eq!(status_pill("working"), (TEAL, TEXT));
        assert_eq!(status_pill("done"), (TEAL, TEXT));
        assert_eq!(status_pill("idle"), (LINE, DIM));
        assert_eq!(status_pill(""), (LINE, DIM));
    }
}
