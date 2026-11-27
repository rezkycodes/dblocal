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

/// The title-bar search button and launcher controls are keyboard-reachable
/// tab stops (`F-PLH-01` AC1).
#[gpui_kit::test]
fn tab_and_enter_reaches_the_title_search_and_launcher_controls(cx: &mut TestAppContext) {
    let (view, _service, cx) = harness(cx);
    // 1) Title-bar search button is the first tab stop and Enter opens the palette.
    let mut opened_palette = false;
    for _ in 0..10 {
        press(cx, "tab");
        if !has_focus(cx) {
            continue;
        }
        press(cx, "enter");
        if view.read_with(cx, |this, _| this.palette_is_open()) {
            opened_palette = true;
            break;
        }
    }
    assert!(
        opened_palette,
        "Tab + Enter must reach the title-bar search button and open the palette"
    );
    press(cx, "escape");
    draw(cx);

    // 2) Walk Shift+Tab backward from the root into the launcher controls at
    //    the end of the tree (status-bar downloads -> launcher Settings ->
    //    launcher Palette -> launcher New instance -> launcher instances ->
    //    launcher quick-create tile). Confirm that the launcher's Settings,
    //    Palette, instance, and quick-create controls are all reachable by key.
    let root_focus = view.read_with(cx, |this, cx| {
        use gpui_kit::Focusable as _;
        this.focus_handle(cx)
    });
    focus(cx, &root_focus);
    let mut saw_settings = false;
    let mut saw_palette = false;
    let mut saw_instance_open = false;
    let mut create_opens = 0usize;
    for _ in 0..12 {
        press(cx, "shift-tab");
        if !has_focus(cx) {
            continue;
        }
        press(cx, "enter");
        if view.read_with(cx, |this, _| this.settings_state().open) {
            saw_settings = true;
            press(cx, "escape");
            continue;
        }
        if view.read_with(cx, |this, _| this.palette_is_open()) {
            saw_palette = true;
            press(cx, "escape");
            continue;
        }
        if view.read_with(cx, |this, _| this.tabs.len()) > 0 {
            saw_instance_open = true;
            cx.update(|_, cx| view.update(cx, |this, cx| this.close_tab(0, cx)));
            draw(cx);
            continue;
        }
        if view.read_with(cx, |this, _| this.create_form().is_some()) {
            create_opens += 1;
            press(cx, "escape");
        }
    }
    assert!(
        saw_settings && saw_palette && saw_instance_open && create_opens >= 2,
        "Shift+Tab across the launcher must reach Settings ({saw_settings}), \
         Palette ({saw_palette}), an instance row ({saw_instance_open}), and \
         both New instance + Quick create ({create_opens})"
    );
}
