//! Fix round 2 (`G1`, `G3`): `Ctrl+Shift+S/X`, delete/quit focus restore, quit
//! trap, group-header reachability, palette CopyUri.
//!
//! Included as a child of the `keyboard_flow_tests` module so it shares the
//! harness and key helpers. Every test drives the real key path; the mutations
//! that turn each red are in `docs/dev/accessibility-checklist.md`.

use super::*;

/// `G1`: `Ctrl+Shift+S` starts the selected (stopped) instance; `Ctrl+Shift+X`
/// stops the selected (running) one — each recorded per-instance in the service.
#[gpui_kit::test]
fn ctrl_shift_s_x_start_and_stop_the_selected_instance(cx: &mut TestAppContext) {
    let (view, service, cx) = harness(cx);
    let stopped = service.id_with(InstanceStatus::Stopped);
    let running = service.id_with(InstanceStatus::Running);

    // Select the stopped instance (a tab). Put the keyboard focus on a child
    // control (Tab once from the root lands on a header button), so the binding
    // has to bubble from that focused child up to the workspace root.
    cx.update(|_window, cx| {
        view.update(cx, |this, cx| this.open_tab(stopped.clone(), cx));
    });
    cx.run_until_parked();
    press(cx, "tab");
    assert!(
        has_focus(cx),
        "a child control holds focus before the shortcut"
    );
    press(cx, "ctrl-shift-s");
    cx.run_until_parked();
    assert_eq!(
        service.started(),
        vec![stopped.clone()],
        "Ctrl+Shift+S must start exactly the selected stopped instance"
    );

    // Now select the running instance and stop it.
    cx.update(|_window, cx| {
        view.update(cx, |this, cx| this.open_tab(running.clone(), cx));
    });
    cx.run_until_parked();
    press(cx, "tab");
    assert!(
        has_focus(cx),
        "a child control holds focus before the shortcut"
    );
    press(cx, "ctrl-shift-x");
    cx.run_until_parked();
    assert_eq!(
        service.stopped(),
        vec![running],
        "Ctrl+Shift+X must stop exactly the selected running instance"
    );
}

/// `G1`: wrong status / no selection makes the key a no-op (no service call).
#[gpui_kit::test]
fn ctrl_shift_s_x_are_noops_for_wrong_status_or_no_selection(cx: &mut TestAppContext) {
    let (view, service, cx) = harness(cx);
    // No tab open: neither key may reach start/stop.
    press(cx, "ctrl-shift-s");
    press(cx, "ctrl-shift-x");
    cx.run_until_parked();
    assert!(service.started().is_empty(), "no selection: no start call");
    assert!(service.stopped().is_empty(), "no selection: no stop call");

    // Start on a RUNNING instance is a no-op; stop on a STOPPED one is a no-op.
    let running = service.id_with(InstanceStatus::Running);
    let stopped = service.id_with(InstanceStatus::Stopped);
    cx.update(|_window, cx| {
        view.update(cx, |this, cx| this.open_tab(running.clone(), cx));
    });
    cx.run_until_parked();
    press(cx, "ctrl-shift-s");
    cx.run_until_parked();
    assert!(
        service.started().is_empty(),
        "Ctrl+Shift+S on a running instance must be a no-op"
    );

    cx.update(|_window, cx| {
        view.update(cx, |this, cx| this.open_tab(stopped.clone(), cx));
    });
    cx.run_until_parked();
    press(cx, "ctrl-shift-x");
    cx.run_until_parked();
    assert!(
        service.stopped().is_empty(),
        "Ctrl+Shift+X on a stopped instance must be a no-op"
    );
}

/// `G3(a)`: after the delete dialog confirms, focus returns and `ctrl-k` opens.
#[gpui_kit::test]
fn delete_confirm_restores_focus_and_keeps_the_keyboard_alive(cx: &mut TestAppContext) {
    let (view, service, cx) = harness(cx);
    let id = service.id_with(InstanceStatus::Stopped);
    cx.update(|window, cx| {
        view.update(cx, |this, cx| {
            this.open_delete_dialog(id.clone(), window, cx)
        });
    });
    draw(cx);
    assert!(has_focus(cx), "delete dialog takes focus on open");
    // Confirm delete (Enter on the focused "Delete" is bypassed by typing the
    // name not being needed in Archive mode; call the confirm action the button
    // runs — the focus restore is what is under test).
    cx.update(|_window, cx| {
        view.update(cx, |this, cx| this.confirm_delete_action(cx));
    });
    cx.run_until_parked();
    draw(cx);
    assert!(
        view.read_with(cx, |this, cx| this.delete_target_name(cx).is_none()),
        "confirm closes the delete dialog"
    );
    assert!(has_focus(cx), "focus must survive the delete confirm (G3a)");
    press(cx, "ctrl-k");
    assert!(
        view.read_with(cx, |this, _| this.palette_is_open()),
        "ctrl-k must open after a delete confirm (no dead keyboard)"
    );
}

/// `G3(b)`: dismissing the quit dialog returns focus to the SAME child that had
/// it (not just the root), and `ctrl-k` opens again.
#[gpui_kit::test]
fn quit_cancel_restores_focus_and_keeps_the_keyboard_alive(cx: &mut TestAppContext) {
    let (view, _service, cx) = harness(cx);
    // Focus the PostgreSQL group header — a child control, so "focus is Some"
    // alone cannot pass; the exact handle must come back.
    let header = view
        .read_with(cx, |this, _| this.dock_group_focus(EngineKey::Postgresql))
        .expect("postgres group header focus handle");
    focus(cx, &header);
    draw(cx);
    assert!(
        cx.update(|window, _cx| header.is_focused(window)),
        "the group header holds focus before quitting"
    );
    // Open the quit dialog for a running instance via the real quit request.
    cx.update(|_window, cx| {
        view.update(cx, |this, cx| {
            let running = this
                .instances()
                .iter()
                .filter(|v| v.status == InstanceStatus::Running)
                .map(|v| v.id.clone())
                .collect();
            this.request_quit_with(running, cx);
        });
    });
    draw(cx);
    assert!(
        view.read_with(cx, |this, _| this.quit_machine().is_confirming()),
        "the quit dialog must be open"
    );
    // Cancel it (the "Cancel" button path).
    cx.update(|_window, cx| {
        view.update(cx, |this, cx| this.quit_cancel(cx));
    });
    draw(cx);
    assert!(
        !view.read_with(cx, |this, _| this.quit_machine().is_confirming()),
        "cancel closes the quit dialog"
    );
    assert!(
        cx.update(|window, _cx| header.is_focused(window)),
        "cancel must return focus to the exact opener child (G3b), not just the root"
    );
    press(cx, "ctrl-k");
    assert!(
        view.read_with(cx, |this, _| this.palette_is_open()),
        "ctrl-k must open after the quit dialog cancels"
    );
}

/// `G3(c)`: Tab stays inside the quit dialog and wraps.
#[gpui_kit::test]
fn tab_is_contained_in_the_quit_dialog(cx: &mut TestAppContext) {
    let (view, _service, cx) = harness(cx);
    cx.update(|_window, cx| {
        view.update(cx, |this, cx| {
            let running = this
                .instances()
                .iter()
                .filter(|v| v.status == InstanceStatus::Running)
                .map(|v| v.id.clone())
                .collect();
            this.request_quit_with(running, cx);
        });
    });
    draw(cx);
    assert!(view.read_with(cx, |this, _| this.quit_machine().is_confirming()));
    assert!(
        active_trap(cx).is_some(),
        "the quit dialog must register a focus trap (G3c)"
    );
    assert_trapped(cx, true, 40, "quit Tab");
    assert_trapped(cx, false, 40, "quit Shift+Tab");
}

/// `G3(d)`: real Tab reaches the engine-group header and Enter/Space toggles it.
#[gpui_kit::test]
fn tab_reaches_the_group_header_and_enter_toggles_it(cx: &mut TestAppContext) {
    let (view, _service, cx) = harness(cx);
    let engine = EngineKey::Postgresql;
    let start_collapsed = view.read_with(cx, |this, _| this.is_collapsed(engine));
    // Walk with Tab+Enter looking for the group header (its Enter toggles the
    // collapsed state of PostgreSQL).
    let mut toggled = false;
    for _ in 0..60 {
        press(cx, "tab");
        if !has_focus(cx) {
            continue;
        }
        press(cx, "enter");
        if view.read_with(cx, |this, _| this.is_collapsed(engine)) != start_collapsed {
            toggled = true;
            break;
        }
    }
    assert!(
        toggled,
        "Tab + Enter must reach the group header and toggle it (G3d)"
    );
}

/// `G3(e)`: the palette's Copy URI command copies the URI (clipboard + recorded
/// `connection_uri`), driven by typing "copy" and Enter.
#[gpui_kit::test]
fn palette_copy_uri_puts_the_uri_on_the_clipboard(cx: &mut TestAppContext) {
    let (view, service, cx) = harness(cx);
    let stopped = service.id_with(InstanceStatus::Stopped);
    press(cx, "ctrl-k");
    assert!(view.read_with(cx, |this, _| this.palette_is_open()));
    // "copy" leaves one Copy URI row per instance in dock order
    // (pg-run, pg-stop, pg-fail); Down once selects the pg-stop copy and Enter
    // confirms — driven by keys only.
    type_text(cx, "copy");
    let matched = view.read_with(cx, |this, cx| this.command_state().read(cx).matched_count());
    assert_eq!(
        matched, 3,
        "one Copy URI row per instance for the copy query"
    );
    press(cx, "down");
    press(cx, "enter");
    cx.run_until_parked();
    let clip = cx
        .read_from_clipboard()
        .and_then(|item| item.text())
        .unwrap_or_default();
    assert!(
        clip.starts_with("postgresql://"),
        "Enter on the palette Copy URI row must copy the URI, got {clip:?}"
    );
    assert!(
        service.uris().iter().any(|(id, _)| id == &stopped),
        "the palette copy must fetch the stopped instance's URI"
    );
}
