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
use super::{Workspace, dark_primary, light_primary, render};

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

/// The bounding box `(x0, y0, x1, y1)` of the newly-added ring pixels, or `None`.
///
/// A ring drawn on all four sides spans the full row (width **and** height); a
/// ring on only the top edge is a thin horizontal band (height ≈ 2 device px).
fn new_ring_bbox(
    unfocused: &Frame,
    focused: &Frame,
    ring: [u8; 3],
) -> Option<(u32, u32, u32, u32)> {
    let (w, h) = focused.dims();
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0u32, 0u32);
    let mut seen = false;
    for y in 0..h {
        for x in 0..w {
            if near(focused.px(x, y), ring, 6) && !near(unfocused.px(x, y), ring, 6) {
                seen = true;
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
            }
        }
    }
    seen.then_some((x0, y0, x1, y1))
}

#[test]
fn a_focused_engine_card_draws_the_theme_ring() {
    for (dark, ring) in [(true, dark_primary()), (false, light_primary())] {
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

/// `G2`: a focused **version row** draws the ring on all four edges, not a thin
/// line on one edge. The scale factor doubles the row's logical height, so a
/// full outline's bbox is tall; a top-edge-only line's bbox is ~2 device px.
#[test]
fn a_focused_version_row_draws_a_full_outline() {
    for (dark, ring) in [(true, dark_primary()), (false, light_primary())] {
        let before = render(dark, "vring-before.png", Workspace::preview_create);
        let after = render(
            dark,
            "vring-after.png",
            Workspace::preview_version_row_focused,
        );
        let scale = after.scale;
        let added = new_ring_pixels(&before, &after, ring);
        let bbox = new_ring_bbox(&before, &after, ring).expect("new ring pixels");
        let height = bbox.3 - bbox.1;
        let width = bbox.2 - bbox.0;
        eprintln!(
            "VRING dark={dark}: added={added} bbox={bbox:?} w={width} h={height} scale={scale}"
        );
        // A version row is 42 logical px tall; a full outline spans at least most
        // of that. A top-edge-only line would be <= 4 device px tall.
        assert!(
            height >= 30 * scale,
            "dark={dark}: version-row ring must span the row height (all four edges); bbox height {height} < {} (top-edge-only mutation)",
            30 * scale
        );
        assert!(
            width >= 200 * scale,
            "dark={dark}: version-row ring must span the row width; bbox width {width}"
        );
        assert!(
            added > 200,
            "dark={dark}: expected a substantial ring ({added})"
        );
    }
}

/// `G2`: a focused **engine-group header** draws the ring on all four edges.
#[test]
fn a_focused_group_header_draws_a_full_outline() {
    for (dark, ring) in [(true, dark_primary()), (false, light_primary())] {
        let before = render(dark, "hring-before.png", Workspace::preview);
        let after = render(
            dark,
            "hring-after.png",
            Workspace::preview_dock_group_focused,
        );
        let scale = after.scale;
        let added = new_ring_pixels(&before, &after, ring);
        let bbox = new_ring_bbox(&before, &after, ring).expect("new ring pixels");
        let height = bbox.3 - bbox.1;
        let width = bbox.2 - bbox.0;
        eprintln!(
            "HRING dark={dark}: added={added} bbox={bbox:?} w={width} h={height} scale={scale}"
        );
        // A group header is 30 logical px tall; a full outline spans most of it.
        assert!(
            height >= 20 * scale,
            "dark={dark}: group-header ring must span the header height (all four edges); bbox height {height} < {} (one-edge mutation)",
            20 * scale
        );
        assert!(
            width >= 100 * scale,
            "dark={dark}: group-header ring must span the header width; bbox width {width}"
        );
        assert!(
            added > 100,
            "dark={dark}: expected a substantial ring ({added})"
        );
    }
}
