//! W4-I D3: a long log line must not wrap into the next row.
//!
//! In the real window a PostgreSQL `LOG: ... listening on IPv4 address` line
//! (about 120+ characters at 1280 px) wrapped to two visual lines while its row
//! kept the single-line height, so the wrapped part overprinted the next line and
//! the text was unreadable. The log list is a `UniformList`, whose rows all share
//! one height, so wrapping must never happen: the row clips and ellipsises, and
//! "Salin log" still copies the complete lines.
//!
//! The fixture is a 300-character line between two short ones. The test finds the
//! log body between the drawn header/footer borders and counts horizontal text
//! bands there: with one line per row and no wrapping there are exactly three,
//! each one line tall, at the configured row pitch. A re-enabled wrap draws the
//! 300-char line as several stacked visual lines inside a single 18 px row; the
//! bands then merge into a tall block (or touch their neighbours), which fails.
//!
//! Included from `tests/render.rs` (uses its `render_sized`, `Build`, `Frame`
//! and the band helpers). Follow-tail is on in the fixture (the buffer is the
//! newest three-line tail, scrolled to the bottom).

use super::frame::Frame;
use super::{DARK_BG, LIGHT_BG, Workspace, log_viewer_body, render_sized};

/// One log row, logical px. Matches `screens/logs.rs` `LOG_ROW_HEIGHT`.
const ROW_H: f32 = 18.0;

const DARK_BORDER: [u8; 3] = [38, 40, 51];
const LIGHT_BORDER: [u8; 3] = [223, 224, 231];

/// Asserts the log body holds `rows` one-line bands at the row pitch, with no
/// band taller than a single line — i.e. nothing wrapped into its neighbour.
fn assert_one_line_rows(
    frame: &Frame,
    border: [u8; 3],
    bg: [u8; 3],
    rows: usize,
    label: &str,
) -> Vec<(u32, u32)> {
    let body = log_viewer_body(frame, border);
    let bands = super::text_bands(frame, body, bg, 14);
    let scale = frame.scale as f32;
    eprintln!("{label}: body={body:?} bands={bands:?} scale={scale}");
    assert_eq!(
        bands.len(),
        rows,
        "{label}: the body must show {rows} one-line log rows, got {} (a wrapped line merges with its neighbour): {bands:?}",
        bands.len()
    );
    // Each band is a single line of text, not a wrapped stack: well under one
    // row height (18 logical px) plus its own glyph height.
    for (start, end) in &bands {
        let h = (*end - *start) as f32 / scale;
        assert!(
            h <= ROW_H,
            "{label}: a log band is {h} logical px tall (> {ROW_H}); the line wrapped inside its fixed-height row"
        );
    }
    // Consecutive bands are one row pitch apart (±2 logical px), and never touch:
    // a wrapped line would push the next band into this one.
    for pair in bands.windows(2) {
        let gap = (pair[1].0 - pair[0].1) as f32 / scale;
        assert!(
            gap > 1.0,
            "{label}: two log bands touch (gap={gap} logical px); a wrapped line overprinted the next row"
        );
    }
    bands
}

#[test]
fn a_long_log_line_does_not_wrap_into_the_next_row() {
    // 1280 px (the review window) and the 900 px minimum width, both themes.
    for (width, height) in [(1280, 820), (900, 600)] {
        for (dark, border, bg) in [
            (true, DARK_BORDER, DARK_BG),
            (false, LIGHT_BORDER, LIGHT_BG),
        ] {
            let frame = render_sized(
                dark,
                &format!("logs-long-line-{width}-{dark}.png"),
                Workspace::preview_logs_long_line,
                width,
                height,
            );
            // Three lines, one per row, none wrapped.
            assert_one_line_rows(
                &frame,
                border,
                bg,
                3,
                &format!("LOGS-LONG width={width} dark={dark}"),
            );
        }
    }
}
