//! The Delete actions: trash glyph and the header button's danger tone.
//!
//! `F-INS-05` draws Delete in the instance header as an icon-only trash in the
//! danger colour, beside Stop / Restart / "Open in…". These tests assert, on
//! the real frame:
//!
//! * the header paints the theme's danger colour as one glyph-sized shape, in
//!   both themes, on the page plane the header sits on;
//! * the Databases row's trash stays neutral, so a disabled row never reads red.
//!
//! A blank or missing icon renders as nothing, so "ink exists where the icon
//! is" is the glyph-present proof at the pixel level; `components` and `assets`
//! unit tests pin the *trash* glyph itself (path and bundled bytes).
//!
//! Included from `tests/render.rs` (uses its `render`, `Workspace`, `token`).

use super::{DOCK, LOGICAL_H, LOGICAL_W, TAB_BOTTOM, Workspace, render, token};

fn near(px: [u8; 4], rgb: [u8; 3], tol: u8) -> bool {
    px[0].abs_diff(rgb[0]) <= tol && px[1].abs_diff(rgb[1]) <= tol && px[2].abs_diff(rgb[2]) <= tol
}

/// Device-pixel bounding box and count of pixels near `rgb` inside a logical rect.
fn ink_bbox(
    frame: &super::frame::Frame,
    rect: (u32, u32, u32, u32),
    rgb: [u8; 3],
    tol: u8,
) -> Option<(u32, u32, u32, u32, usize)> {
    let (x0, y0, x1, y1) = frame.rect(rect);
    let (mut bx0, mut by0, mut bx1, mut by1) = (u32::MAX, u32::MAX, 0u32, 0u32);
    let mut count = 0usize;
    for y in y0..y1 {
        for x in x0..x1 {
            if near(frame.px(x, y), rgb, tol) {
                count += 1;
                bx0 = bx0.min(x);
                by0 = by0.min(y);
                bx1 = bx1.max(x);
                by1 = by1.max(y);
            }
        }
    }
    (count > 0).then_some((bx0, by0, bx1, by1, count))
}

/// `F-INS-05`: the header Delete is an icon-only trash in the danger colour, in
/// both themes. The accessible name "Delete" is asserted in `a11y_tree.rs`
/// (`the_header_delete_control_is_named`); here the pixels prove
/// the glyph is drawn, red, glyph-sized and not a blank or a text label.
#[test]
fn header_delete_is_a_danger_trash_glyph_in_both_themes() {
    let header = (DOCK, TAB_BOTTOM + 2, LOGICAL_W, TAB_BOTTOM + 64);
    // Render both themes before asserting, so a failure still leaves both
    // frames on disk.
    let frames: [(bool, &str, super::frame::Frame); 2] = [
        (
            true,
            "dark",
            render(true, "header-delete-dark.png", Workspace::preview),
        ),
        (
            false,
            "light",
            render(false, "header-delete-light.png", Workspace::preview),
        ),
    ];
    for (dark, label, frame) in &frames {
        let danger = token(*dark, "danger.background");
        let (x0, y0, x1, y1, count) = ink_bbox(frame, header, danger, 18).unwrap_or_else(|| {
            panic!("{label}: header Delete must paint the danger colour; none found in {header:?}")
        });
        let scale = frame.scale;
        let (w, h) = (x1 - x0 + 1, y1 - y0 + 1);
        eprintln!(
            "HEADER-DELETE {label}: ink={count} bbox=({x0},{y0},{x1},{y1}) w={w} h={h} scale={scale}"
        );
        // A blank icon paints nothing; a stroke glyph about 14-16 logical px
        // square leaves a few dozen device pixels at least.
        assert!(
            count >= 20 * (scale as usize).pow(2),
            "{label}: the trash glyph is blank or missing ({count} danger pixels)"
        );
        // Icon-only: a glyph-sized square, not a word. A text label would be
        // far wider than it is tall.
        assert!(
            w >= 10 * scale && w <= 24 * scale && h >= 10 * scale && h <= 24 * scale,
            "{label}: the danger ink is {w}x{h} device px, expected one icon-sized glyph"
        );
    }
}

/// Pixels inside a logical rect that match none of `bgs` — glyph ink on a
/// surface, excluding the surfaces themselves.
fn ink_outside(
    frame: &super::frame::Frame,
    rect: (u32, u32, u32, u32),
    bgs: &[[u8; 3]],
    tol: u8,
) -> usize {
    let (x0, y0, x1, y1) = frame.rect(rect);
    let mut hits = 0;
    for y in y0..y1 {
        for x in x0..x1 {
            let p = frame.px(x, y);
            if !bgs.iter().any(|bg| near(p, *bg, tol)) {
                hits += 1;
            }
        }
    }
    hits
}

/// The Databases row keeps the neutral ghost look: no danger anywhere in the
/// tab (so the disabled system row never reads red), and the trash still paints
/// in the action column.
#[test]
fn databases_row_delete_stays_neutral_in_both_themes() {
    let body = (DOCK, TAB_BOTTOM + 70, LOGICAL_W, LOGICAL_H - 60);
    // The Database table caps at 1080px with a 24px gutter, so its right edge
    // and the 80px action column sit at the far right of the page.
    let action = (1170, TAB_BOTTOM + 80, LOGICAL_W - 20, TAB_BOTTOM + 460);
    for (dark, label) in [(true, "dark"), (false, "light")] {
        let frame = render(
            dark,
            &format!("delete-row-{label}.png"),
            Workspace::preview_databases,
        );
        let danger = token(dark, "danger.background");
        let page = token(dark, "background");
        let card = token(dark, "group_box.background");
        let danger_ink = frame.count_near(body, danger, 20);
        let glyph_ink = ink_outside(&frame, action, &[page, card], 12);
        eprintln!("DELETE-ROW {label}: danger_ink={danger_ink} action_glyph_ink={glyph_ink}");
        assert!(
            danger_ink < 200,
            "{label}: the trash on a database row must stay neutral, found {danger_ink} danger pixels"
        );
        assert!(
            glyph_ink > 60,
            "{label}: expected the row trash glyph to paint in the action column ({glyph_ink})"
        );
    }
}
