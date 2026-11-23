//! W4-I D2: the command palette's panel height follows its content.
//!
//! Found in the real window during the W4-G review: the 560 px palette panel was
//! drawn as a full-height white column from its first row to the window bottom,
//! hiding the page behind the scrim even with a single row left after a filter.
//! The overlay is a flex row with `justify_center` and no `items_start`, so the
//! panel was **stretched** on the cross axis (a flex row's default `align-items`
//! is `stretch`), and it filled everything below `pt(90)`.
//!
//! The measurement is layout-derived, not a magic y. The panel draws a 1 px
//! border in the theme `border` colour, and **the page behind the scrim does
//! not**: the 45% scrim darkens every page pixel, so the only rows whose centre
//! band is exactly the theme border are the panel's own borders (its top edge,
//! its search-field divider, its bottom edge). The first and last such rows are
//! therefore the panel's drawn top and bottom. The test asserts the panel leaves
//! a large gap above the window bottom and that a one-row palette is shorter than
//! the full list — i.e. the height follows the content.
//!
//! Included from `tests/render.rs` (uses its `render`, `Build`, `Frame` and the
//! committed theme border tokens).

use super::frame::Frame;
use super::{LOGICAL_H, LOGICAL_W, Workspace, render};

/// `border` from the committed theme JSON (`dblocal-dark.json` / `-light.json`).
const DARK_BORDER: [u8; 3] = [0x30, 0x32, 0x3f];
const LIGHT_BORDER: [u8; 3] = [0xcf, 0xd0, 0xd9];

fn exact(px: [u8; 4], rgb: [u8; 3]) -> bool {
    px[0] == rgb[0] && px[1] == rgb[1] && px[2] == rgb[2]
}

/// `(top, bottom)` device rows of the palette panel, plus the scale.
///
/// Scans the panel's middle band (its centre third, clear of the rounded
/// corners) for rows that are mostly the exact theme border colour; the first and
/// last such rows are the panel's top and bottom border. Rows are coalesced so a
/// 2-device-px border counts once.
fn panel_span(frame: &Frame, border: [u8; 3], label: &str) -> ((u32, u32), u32) {
    let s = frame.scale;
    let x0 = (LOGICAL_W / 2 - 90) * s;
    let x1 = (LOGICAL_W / 2 + 90) * s;
    let (w, h) = frame.dims();
    let x1 = x1.min(w);
    let span = (x1 - x0).max(1);

    let mut border_rows: Vec<u32> = Vec::new();
    for y in 0..h {
        let hits = (x0..x1).filter(|x| exact(frame.px(*x, y), border)).count();
        if hits * 100 / span as usize >= 60 {
            border_rows.push(y);
        }
    }
    assert!(
        border_rows.len() >= 2,
        "{label}: could not find the palette panel's borders in the centre band \
         (found {} border rows); is the panel drawn?",
        border_rows.len()
    );
    let top = *border_rows.first().expect("non-empty");
    let bottom = *border_rows.last().expect("non-empty");
    ((top, bottom), s)
}

/// The panel's drawn height and the gap to the window bottom, in logical px.
fn measure(frame: &Frame, border: [u8; 3], label: &str) -> (u32, u32) {
    let ((top, bottom), scale) = panel_span(frame, border, label);
    let height = (bottom.saturating_sub(top)) / scale;
    let gap = LOGICAL_H.saturating_sub(bottom / scale);
    eprintln!(
        "PALETTE-HEIGHT {label}: top={top} bottom={bottom} panel_h_logical={height} \
         gap_to_window_bottom={gap}"
    );
    // The panel starts below the overlay's pt(90) inset (its top border sits a
    // pixel above the first row, so 85 is the floor).
    assert!(
        top >= 85 * scale,
        "{label}: the palette panel must start below the pt(90) inset (top={top} device)"
    );
    // The defect filled from the first row to the window bottom. A content-sized
    // panel leaves a large gap above the status bar.
    assert!(
        gap > 150,
        "{label}: the palette panel stretches toward the window bottom \
         (panel_h={height} logical px, gap={gap}); it must follow its content"
    );
    (height, gap)
}

#[test]
fn the_palette_panel_height_follows_its_content() {
    for (dark, border) in [(true, DARK_BORDER), (false, LIGHT_BORDER)] {
        let one = render(
            dark,
            "palette-height-one-row.png",
            Workspace::preview_palette_one_row,
        );
        let (one_h, _) = measure(&one, border, &format!("dark={dark} one-row"));

        let all = render(
            dark,
            "palette-height-all-rows.png",
            Workspace::preview_palette,
        );
        let (all_h, _) = measure(&all, border, &format!("dark={dark} all-rows"));

        // A filtered list with one row must draw a shorter panel than the full
        // list: the height follows the content, it is not a fixed window slice.
        assert!(
            one_h + 40 < all_h,
            "dark={dark}: a one-row palette ({one_h}) must be shorter than the full list \
             ({all_h}); the panel height does not follow its content"
        );
    }
}
