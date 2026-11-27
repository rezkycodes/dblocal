//! The focus ring shows for keyboard focus only, never after a mouse click.
//!
//! Included as a child of `keyboard_flow_tests` so it shares the harness and the
//! key helpers. A click focuses a row (it is a tab stop), but gpui tracks the
//! last input modality: the ring must follow the keyboard, as the kit's own
//! `focus_visible` does. Every ring site in `crates/gui` goes through
//! `components::focus_ring_visible`, and the ring leaves a zero-size
//! `focus-ring-marker` behind that these tests find with `debug_bounds`.

use super::*;
use gpui_kit::{Modifiers, Pixels, Point, point, px};

const MARKER: &str = "focus-ring-marker";

/// Where the ring is drawn now, or `None` when no ring is on screen.
fn ring_origin(cx: &mut VisualTestContext) -> Option<Point<Pixels>> {
    cx.debug_bounds(MARKER).map(|b| b.origin)
}

/// One Tab key press (real key dispatch, so the input modality is keyboard) and
/// the ring origin it leaves, or `None` when nothing is focused.
fn tab_stop(cx: &mut VisualTestContext) -> Option<Option<Point<Pixels>>> {
    press(cx, "tab");
    has_focus(cx).then(|| ring_origin(cx))
}

/// A real left click `inset` px inside the element whose ring marker sat at
/// `origin`: mouse move, down and up, as a pointer would send them.
fn click_at(cx: &mut VisualTestContext, origin: Point<Pixels>, inset: f32) {
    let at = point(origin.x + px(inset), origin.y + px(inset));
    cx.simulate_mouse_move(at, None, Modifiers::none());
    cx.simulate_click(at, Modifiers::none());
    draw(cx);
}

/// Opens two tabs so the screen has a tab strip, the dock rows and group headers.
fn open_two_tabs(view: &Entity<Workspace>, service: &QuietService, cx: &mut VisualTestContext) {
    let stopped = service.id_with(InstanceStatus::Stopped);
    let running = service.id_with(InstanceStatus::Running);
    cx.update(|_window, cx| {
        view.update(cx, |this, cx| {
            this.open_tab(stopped.clone(), cx);
            this.open_tab(running.clone(), cx);
            this.select_tab(0, cx);
        });
    });
    cx.run_until_parked();
    draw(cx);
}

/// Walks Tab from the top and collects the ring origin of every gui-owned ring stop.
fn keyboard_walk(cx: &mut VisualTestContext) -> Vec<Point<Pixels>> {
    let mut seen: Vec<Point<Pixels>> = Vec::new();
    for _ in 0..60 {
        // Kit buttons draw their own ring and leave no marker: skip them here.
        let Some(Some(origin)) = tab_stop(cx) else {
            continue;
        };
        if !seen.contains(&origin) {
            seen.push(origin);
        }
    }
    seen
}

/// Tab / arrow focus paints the ring on every ring site of the main screen: the
/// title search field, dock group headers and rows, and the instance tabs.
#[gpui_kit::test]
fn keyboard_focus_paints_the_ring_on_every_stop(cx: &mut TestAppContext) {
    let (view, service, cx) = harness(cx);
    open_two_tabs(&view, &service, cx);
    let stops = keyboard_walk(cx);
    eprintln!("RING-STOPS {stops:?}");
    // Search field first, in the title bar.
    assert!(
        stops.iter().any(|p| p.y < px(34.)),
        "the title search field must paint its ring on keyboard focus"
    );
    // Dock group headers and rows share the left column; tabs sit right of it.
    assert!(
        stops
            .iter()
            .filter(|p| p.y > px(34.) && p.x < px(280.))
            .count()
            >= 2,
        "dock headers and rows must paint their ring on keyboard focus"
    );
    assert!(
        stops
            .iter()
            .any(|p| p.y > px(34.) && p.y < px(100.) && p.x > px(280.)),
        "an instance tab must paint its ring on keyboard focus"
    );
}

/// A mouse click on the same ring sites focuses them but paints no ring; the
/// next Tab brings the ring back.
#[gpui_kit::test]
fn a_mouse_click_paints_no_ring_but_the_next_tab_does(cx: &mut TestAppContext) {
    let (view, service, cx) = harness(cx);
    open_two_tabs(&view, &service, cx);
    let mut stops = keyboard_walk(cx);
    // Search opens the palette and covers the screen: click it last.
    stops.sort_by(|a, b| b.y.partial_cmp(&a.y).unwrap_or(std::cmp::Ordering::Equal));
    let mut clicked = 0usize;
    for origin in stops {
        click_at(cx, origin, 6.0);
        assert!(
            ring_origin(cx).is_none(),
            "a mouse click at {origin:?} must not paint a focus ring"
        );
        clicked += 1;
        // The search field opens the palette; close it so Tab walks the screen.
        if view.read_with(cx, |this, _| this.palette_is_open()) {
            press(cx, "escape");
        }
        // The keyboard takes over: within a few Tab presses a gui-owned ring
        // site is focused again and its ring is back.
        let ring_back = (0..12).any(|_| matches!(tab_stop(cx), Some(Some(_))));
        assert!(
            ring_back,
            "Tab after a click at {origin:?} must paint the ring again (focus={})",
            has_focus(cx)
        );
    }
    assert!(
        clicked >= 4,
        "expected several ring sites, clicked {clicked}"
    );
}
