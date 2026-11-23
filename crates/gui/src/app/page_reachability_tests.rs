//! `F4`: the main screen (sidebar rows, instance tabs) is keyboard-reachable.
//!
//! Included as a child of the `keyboard_flow_tests` module so it shares the
//! harness and key helpers. Split out to keep `keyboard_flow_tests.rs` under the
//! 800-line limit.

use super::*;

/// `F4`: the sidebar instance rows are keyboard-reachable and Enter opens one.
///
/// No tabs are open. Walking the main screen with real Tab and pressing Enter at
/// each stop must, at some point, open an instance tab — which only happens when
/// a sidebar row is focused and activated. Before `F4` the rows had no focus
/// handle and this never opened a tab.
#[gpui_kit::test]
fn tab_and_enter_reaches_a_sidebar_row_and_opens_it(cx: &mut TestAppContext) {
    let (view, _service, cx) = harness(cx);
    assert_eq!(
        view.read_with(cx, |this, _| this.tabs.len()),
        0,
        "start with no open tabs"
    );
    let mut opened = false;
    for _ in 0..60 {
        press(cx, "tab");
        if !has_focus(cx) {
            continue;
        }
        press(cx, "enter");
        if view.read_with(cx, |this, _| this.tabs.len()) > 0 {
            opened = true;
            break;
        }
    }
    assert!(
        opened,
        "Tab + Enter on the main screen must reach a sidebar instance row and open it (F4)"
    );
}

/// `F4`: an open instance tab is a keyboard tab stop and Enter selects it.
///
/// Two tabs are open with the FIRST selected. Walking the screen with Tab and
/// pressing Enter at each stop must, at some point, select the other tab — which
/// only happens when an instance tab is focused and activated.
#[gpui_kit::test]
fn tab_and_enter_selects_an_instance_tab(cx: &mut TestAppContext) {
    let (view, service, cx) = harness(cx);
    let stopped = service.id_with(InstanceStatus::Stopped);
    let running = service.id_with(InstanceStatus::Running);
    cx.update(|_window, cx| {
        view.update(cx, |this, cx| {
            this.open_tab(stopped.clone(), cx);
            this.open_tab(running.clone(), cx);
            // Select the first tab so switching to the second is observable.
            this.select_tab(0, cx);
        });
    });
    cx.run_until_parked();
    assert_eq!(view.read_with(cx, |this, _| this.tabs.len()), 2);
    let start = view.read_with(cx, |this, _| this.tabs.selected_index());
    let mut switched = false;
    for _ in 0..80 {
        press(cx, "tab");
        if !has_focus(cx) {
            continue;
        }
        press(cx, "enter");
        if view.read_with(cx, |this, _| this.tabs.selected_index()) != start {
            switched = true;
            break;
        }
    }
    assert!(
        switched,
        "Tab + Enter must reach an instance tab and select it (F4)"
    );
}
