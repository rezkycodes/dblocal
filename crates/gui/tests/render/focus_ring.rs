//! `F3`: the focused create-dialog engine card draws the theme **focus ring**.
//!
//! The earlier first-session test only proved the card takes focus, not that it
//! draws a visible indicator. This renders the real create dialog twice — once
//! with the PostgreSQL card focused and once with nothing focused — and counts
//! pixels that are the theme `ring` colour in the focused frame but were **not**
//! ring-coloured at the same coordinate in the unfocused frame. That isolates
//! the newly drawn ring from the many other places the theme's primary colour
//! legitimately appears (badges, buttons, the selected check).
//!
//! Included from `tests/render.rs` (uses its `render`, `Build`, `Frame`).

use super::frame::Frame;
use super::{DARK_PRIMARY, LIGHT_PRIMARY, Workspace, render};

fn near(px: [u8; 4], rgb: [u8; 3], tol: u8) -> bool {
    px[0].abs_diff(rgb[0]) <= tol && px[1].abs_diff(rgb[1]) <= tol && px[2].abs_diff(rgb[2]) <= tol
}

/// Pixels that are the ring colour in `focused` but not at the same point in
/// `unfocused` — i.e. the ring the focus indicator added.
fn new_ring_pixels(unfocused: &Frame, focused: &Frame, ring: [u8; 3]) -> usize {
    let (w, h) = focused.dims();
    let mut hits = 0;
    for y in 0..h {
        for x in 0..w {
            if near(focused.px(x, y), ring, 6) && !near(unfocused.px(x, y), ring, 6) {
                hits += 1;
            }
        }
    }
    hits
}

#[test]
fn a_focused_engine_card_draws_the_theme_ring() {
    for (dark, ring) in [(true, DARK_PRIMARY), (false, LIGHT_PRIMARY)] {
        let before = render(dark, "ring-before.png", Workspace::preview_create);
        let after = render(
            dark,
            "ring-after.png",
            Workspace::preview_create_engine_focused,
        );
        let added = new_ring_pixels(&before, &after, ring);
        // Diagnostics: how many pixels changed at all, and how many ring pixels
        // each frame has.
        let (w, h) = after.dims();
        let mut changed = 0;
        for y in 0..h {
            for x in 0..w {
                if before.px(x, y) != after.px(x, y) {
                    changed += 1;
                }
            }
        }
        eprintln!("RING dark={dark}: new ring pixels={added}, changed={changed} (ring={ring:?})");
        // A card border is a few hundred device pixels at scale 2; 200 is a low
        // floor that a missing ring (the mutation) cannot reach.
        assert!(
            added > 200,
            "dark={dark}: focusing an engine card must draw new theme-ring pixels (got {added})"
        );
    }
}
