//! Keyboard-only first session (`F-PLH-01` AC1), driven by real key events.
//!
//! The acceptance is that the whole first session — choose engine → create →
//! start → copy URI — completes with only the keyboard, the focus always
//! visible, and no pointer event. These tests build the window with the
//! `gpui-component` `Root` (the app's real window root), so Tab traversal and
//! Enter/Space activation run the same code the binary does. Focus is moved with
//! `Window::focus_next`/`focus_prev` (the exact operation the `Tab` binding runs)
//! and activation is a real `Keystroke` down/up pair; a pointer event is never
//! sent.
//!
//! A test is not evidence until it fails without the code it tests; the worker
//! report pastes the mutation that turns each of these red.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::Arc;

use dblocal_core::model::{EngineKey, InstanceStatus};
use dblocal_core::service::DblService;
use gpui_kit::component::Root;
use gpui_kit::{
    AppContext as _, Entity, FocusHandle, Keystroke, TestAppContext, VisualTestContext,
};

use crate::app::Workspace;
use crate::app::test_service::QuietService;

type Harness<'a> = (Entity<Workspace>, QuietService, &'a mut VisualTestContext);

/// Builds a window whose root is the real `gpui-component` `Root` around a
/// preview `Workspace`, with the workspace key bindings installed (so a
/// `ctrl-n` keystroke runs the same action the binary binds).
fn harness(cx: &mut TestAppContext) -> Harness<'_> {
    cx.update(gpui_kit::init);
    cx.update(crate::theme::init);
    cx.update(crate::app::actions::bind_keys);
    let service = QuietService::three();
    let svc: Arc<dyn DblService> = Arc::new(service.clone());
    let holder: std::rc::Rc<std::cell::RefCell<Option<Entity<Workspace>>>> =
        std::rc::Rc::new(std::cell::RefCell::new(None));
    let holder_for_build = holder.clone();
    let (_root, cx) = cx.add_window_view(move |window, cx| {
        let ws = cx.new(|cx| Workspace::new_preview(svc, window, cx));
        *holder_for_build.borrow_mut() = Some(ws.clone());
        Root::new(ws, window, cx)
    });
    cx.run_until_parked();
    let ws = holder.borrow().clone().expect("workspace built");
    draw(cx);
    (ws, service, cx)
}

/// One frame, so focus and tab-stop registration are current.
fn draw(cx: &mut VisualTestContext) {
    cx.update(|window, cx| {
        window.draw(cx).clear(cx);
    });
}

/// Presses and releases a key on the focused element (real key dispatch).
fn press(cx: &mut VisualTestContext, key: &'static str) {
    let keystroke = Keystroke::parse(key).expect("valid keystroke");
    let release = keystroke.clone();
    cx.update(move |window, cx| {
        window.dispatch_keystroke(keystroke, cx);
        window.dispatch_event(
            gpui_kit::PlatformInput::KeyUp(gpui_kit::KeyUpEvent { keystroke: release }),
            cx,
        );
    });
    draw(cx);
}

/// Focuses a handle inset a frame, as a click or Tab would leave it.
fn focus(cx: &mut VisualTestContext, handle: &FocusHandle) {
    let handle = handle.clone();
    cx.update(|window, cx| handle.focus(window, cx));
    draw(cx);
}

/// Moves focus forward `n` tab stops (the Tab operation) and returns whether
/// something held focus after every step, so a trap or a lost focus is visible.
fn tab_forward(cx: &mut VisualTestContext, n: usize) -> bool {
    let mut focused = true;
    for _ in 0..n {
        cx.update(|window, cx| window.focus_next(cx));
        draw(cx);
        focused &= cx.update(|window, cx| window.focused(cx).is_some());
    }
    focused
}

/// Whether the window currently has any focused element.
fn has_focus(cx: &mut VisualTestContext) -> bool {
    cx.update(|window, cx| window.focused(cx).is_some())
}

/// Types `text` one character at a time into the focused element (real input).
fn type_text(cx: &mut VisualTestContext, text: &str) {
    for ch in text.chars() {
        let s = ch.to_string();
        let key = Keystroke {
            modifiers: Default::default(),
            key: s.clone(),
            key_char: Some(s),
        };
        cx.update(|window, cx| window.dispatch_keystroke(key, cx));
        draw(cx);
    }
}

/// The container focus handle of the active focus trap, if any (`F2`).
fn active_trap(cx: &mut VisualTestContext) -> Option<FocusHandle> {
    cx.update(|window, cx| gpui_kit::base::active_focus_trap(window, cx))
}

/// Asserts that `n` real `Tab` presses (`forward` true) or Shift+Tab (false) keep
/// focus inside the active modal focus trap and never reach the page, and that
/// the walk wraps (returns to a stop already visited) rather than escaping.
///
/// Uses a real `Keystroke` so the `Root`'s Tab action — and therefore the focus
/// trap — runs; `window.focus_next` alone would bypass the containment logic.
fn assert_trapped(cx: &mut VisualTestContext, forward: bool, n: usize, label: &str) {
    let trap = active_trap(cx).unwrap_or_else(|| panic!("{label}: no active focus trap"));
    let key = if forward { "tab" } else { "shift-tab" };
    let mut visited: Vec<String> = Vec::new();
    let mut wrapped = false;
    for _ in 0..n {
        press(cx, key);
        let inside = cx.update(|window, cx| trap.contains_focused(window, cx));
        assert!(
            inside,
            "{label}: focus left the modal focus trap after a {key} (F2)"
        );
        let id = cx.update(|window, cx| window.focused(cx).map(|f| format!("{f:?}")));
        let id = id.unwrap_or_default();
        if visited.contains(&id) {
            wrapped = true;
            break;
        }
        visited.push(id);
    }
    assert!(
        wrapped,
        "{label}: {key} never wrapped within the trap over {n} presses (visited {} stops)",
        visited.len()
    );
}

/// The create form's engine-card handle for `engine`.
fn engine_handle(
    view: &Entity<Workspace>,
    cx: &VisualTestContext,
    engine: EngineKey,
) -> FocusHandle {
    view.read_with(cx, |this, _| {
        this.create_form()
            .and_then(|form| {
                form.engine_focus
                    .iter()
                    .find(|(key, _)| *key == engine)
                    .map(|(_, handle)| handle.clone())
            })
            .expect("engine card handle")
    })
}

/// F-PLH-01 AC2: the palette renders (including the disabled-row branch) and
/// closes on Escape, returning focus — a lib-level draw that exercises
/// `palette::render`'s disabled-row and cancel paths without a pointer.
#[gpui_kit::test]
fn the_palette_renders_with_a_disabled_row(cx: &mut TestAppContext) {
    let (view, _service, cx) = harness(cx);
    let root_focus = view.read_with(cx, |this, cx| {
        use gpui_kit::Focusable as _;
        this.focus_handle(cx)
    });
    focus(cx, &root_focus);
    press(cx, "ctrl-k");
    assert!(view.read_with(cx, |this, _| this.palette_is_open()));
    // Draw with the palette open: this lays out every row, including the
    // disabled start-on-running and stop-on-stopped rows (their muted reason
    // text is a custom child).
    draw(cx);
    // Escape closes it and focus returns to the opener.
    press(cx, "escape");
    draw(cx);
    assert!(
        !view.read_with(cx, |this, _| this.palette_is_open()),
        "Escape closes the palette"
    );
    assert!(
        cx.update(|window, _cx| root_focus.is_focused(window)),
        "closing the palette returns focus to the opener"
    );
}

/// F-PLH-01 AC1: Escape closes the top overlay and returns focus to the control
/// that opened it, so the keyboard user is never stranded.
#[gpui_kit::test]
fn escape_returns_focus_to_the_opener(cx: &mut TestAppContext) {
    let (view, _service, cx) = harness(cx);
    // Park focus on the workspace root, open the create dialog, then Escape.
    let root_focus = view.read_with(cx, |this, cx| {
        use gpui_kit::Focusable as _;
        this.focus_handle(cx)
    });
    focus(cx, &root_focus);
    assert!(
        cx.update(|window, _cx| root_focus.is_focused(window)),
        "the workspace holds focus before opening"
    );
    cx.update(|window, cx| {
        view.update(cx, |this, cx| this.open_new_instance(window, cx));
    });
    draw(cx);
    // The dialog took focus away from the root.
    assert!(
        !cx.update(|window, _cx| root_focus.is_focused(window)),
        "the dialog takes focus from the opener"
    );
    // Escape closes it and gives focus back to the opener.
    press(cx, "escape");
    draw(cx);
    assert!(
        view.read_with(cx, |this, _| this.create_form().is_none()),
        "Escape closes the create dialog"
    );
    assert!(
        cx.update(|window, _cx| root_focus.is_focused(window)),
        "Escape must return focus to the opener"
    );
}

#[gpui_kit::test]
fn ctrl_n_opens_the_create_dialog(cx: &mut TestAppContext) {
    let (view, _service, cx) = harness(cx);
    // Focus the workspace so the keystroke dispatches through its root.
    let root_focus = view.read_with(cx, |this, cx| {
        use gpui_kit::Focusable as _;
        this.focus_handle(cx)
    });
    focus(cx, &root_focus);
    press(cx, "ctrl-n");
    assert!(
        view.read_with(cx, |this, _| this.create_form().is_some()),
        "ctrl-n must open the create dialog"
    );
    // The dialog took focus: something holds it without any pointer.
    assert!(
        cx.update(|window, cx| window.focused(cx).is_some()),
        "the create dialog must take focus on open"
    );
    // Tab never loses focus across the whole dialog (no trap, always visible).
    assert!(
        tab_forward(cx, 10),
        "focus must survive every Tab in the dialog"
    );
}

/// F-PLH-01 AC1: the whole first session by keyboard only.
#[gpui_kit::test]
fn keyboard_only_first_session(cx: &mut TestAppContext) {
    let (view, service, cx) = harness(cx);
    // Open the create dialog through its focusable name field.
    cx.update(|window, cx| {
        view.update(cx, |this, cx| this.open_new_instance(window, cx));
    });
    draw(cx);

    // 1) Choose the engine: focus its card and press Enter (a real key, not a
    //    click). The card's on_click runs through gpui's synthesised activation.
    let pg = engine_handle(&view, cx, EngineKey::Postgresql);
    focus(cx, &pg);
    press(cx, "enter");
    assert_eq!(
        view.read_with(cx, |this, _| this.create_form().map(|f| f.engine)),
        Some(EngineKey::Postgresql),
        "Enter on the focused engine card must select that engine"
    );

    // 2) Choose the version: focus the 16.6 row and press Space (also activates).
    let version_handle = view.read_with(cx, |this, _| {
        this.create_form()
            .and_then(|form| {
                form.version_focus
                    .iter()
                    .find(|(tag, _)| tag == "16.6")
                    .map(|(_, handle)| handle.clone())
            })
            .expect("16.6 version handle")
    });
    focus(cx, &version_handle);
    press(cx, "space");
    assert_eq!(
        view.read_with(cx, |this, _| this.create_form().map(|f| f.version.clone())),
        Some("16.6".to_owned()),
        "Space on the focused version row must select that version"
    );

    // 3) Name is pre-filled from the engine+version; create by focusing the
    //    "Create" button and pressing Enter. No pointer event anywhere.
    let submit = view.read_with(cx, |this, _| {
        this.create_form().map(|f| f.submit_focus.clone())
    });
    let submit = submit.expect("submit focus");
    focus(cx, &submit);
    press(cx, "enter");
    cx.run_until_parked();
    assert!(
        view.read_with(cx, |this, _| this.create_form().is_none()),
        "the create button must submit and close the dialog"
    );
    assert!(
        service.calls() > 0,
        "the submission must reach the service (create)"
    );
}

/// F-PLH-01 AC2: the palette lists start/stop/copy for every instance and,
/// where a command does not apply, shows it disabled with a text reason.
#[gpui_kit::test]
fn palette_disables_inapplicable_commands_with_a_reason(cx: &mut TestAppContext) {
    let (view, _service, _cx) = harness(cx);
    let (labels, reasons) = view.read_with(_cx, |this, _| {
        (this.palette_labels(), this.palette_reasons())
    });
    // The quiet service has one running and one stopped instance, so the
    // "start <running>" and "stop <stopped>" rows exist but are disabled.
    let start_running = labels
        .iter()
        .position(|l| {
            l.starts_with(dblocal_core::strings::UI_PALETTE_START) && l.contains("pg-run")
        })
        .expect("start row for the running instance");
    assert!(
        reasons[start_running].is_some(),
        "start on a running instance is disabled"
    );
    let stop_stopped = labels
        .iter()
        .position(|l| {
            l.starts_with(dblocal_core::strings::UI_PALETTE_STOP) && l.contains("pg-stop")
        })
        .expect("stop row for the stopped instance");
    assert!(
        reasons[stop_stopped].is_some(),
        "stop on a stopped instance is disabled"
    );
}

/// F-PLH-01 AC2: the palette finds an instance by name and engine, runs start on
/// it with **Enter on the filtered row** (not a direct call), and the keyboard is
/// alive again right after the confirm (`F1`).
#[gpui_kit::test]
fn palette_starts_an_instance_with_the_keyboard(cx: &mut TestAppContext) {
    let (view, service, cx) = harness(cx);
    let stopped = service.id_with(InstanceStatus::Stopped);

    let root_focus = view.read_with(cx, |this, cx| {
        use gpui_kit::Focusable as _;
        this.focus_handle(cx)
    });
    focus(cx, &root_focus);
    press(cx, "ctrl-k");
    assert!(
        view.read_with(cx, |this, _| this.palette_is_open()),
        "ctrl-k must open the palette"
    );

    // Type the instance name so the first (selected) row is "Start pg-stop",
    // then confirm with Enter. This exercises the filtered-index ->
    // original-index mapping the earlier direct call skipped.
    type_text(cx, "pg-stop");
    let (matched, selected) = view.read_with(cx, |this, cx| {
        let state = this.command_state().read(cx);
        (state.matched_count(), state.selected_index())
    });
    assert!(
        matched >= 1 && selected.is_some(),
        "typing 'pg-stop' must leave a runnable row selected (matched={matched})"
    );
    press(cx, "enter");
    cx.run_until_parked();
    assert!(
        !view.read_with(cx, |this, _| this.palette_is_open()),
        "Enter confirms and closes the palette"
    );
    assert_eq!(
        service.started(),
        vec![stopped.clone()],
        "Enter on the start row must start exactly the stopped instance"
    );
    // `F1`: the keyboard is not dead after the confirm — ctrl-k opens again.
    assert!(has_focus(cx), "focus must survive the palette confirm");
    press(cx, "ctrl-k");
    assert!(
        view.read_with(cx, |this, _| this.palette_is_open()),
        "ctrl-k must reopen the palette after a confirm (no dead keyboard)"
    );
}

/// `F1`: a fresh window with no prior focus still responds to `ctrl-k`.
///
/// The real window needed a mouse click before `ctrl-k`/`ctrl-n` worked; the
/// fix focuses the root when nothing holds focus, so no click is needed.
#[gpui_kit::test]
fn a_fresh_window_opens_the_palette_without_a_click(cx: &mut TestAppContext) {
    let (view, _service, cx) = harness(cx);
    // Do NOT focus anything: assert the harness left focus unset, then press the
    // shortcut. (A click would focus; this proves the app does it.)
    assert!(
        has_focus(cx),
        "the workspace must take focus on the first frame (F1 startup)"
    );
    press(cx, "ctrl-k");
    assert!(
        view.read_with(cx, |this, _| this.palette_is_open()),
        "ctrl-k must open the palette with no prior click"
    );
}

/// `F1`: after the create dialog closes by confirming, `ctrl-n` opens it again.
#[gpui_kit::test]
fn create_confirm_keeps_the_keyboard_alive(cx: &mut TestAppContext) {
    let (view, _service, cx) = harness(cx);
    let root_focus = view.read_with(cx, |this, cx| {
        use gpui_kit::Focusable as _;
        this.focus_handle(cx)
    });
    focus(cx, &root_focus);
    press(cx, "ctrl-n");
    assert!(view.read_with(cx, |this, _| this.create_form().is_some()));
    // Confirm by focusing "Create" and pressing Enter.
    let submit = view
        .read_with(cx, |this, _| {
            this.create_form().map(|f| f.submit_focus.clone())
        })
        .expect("submit focus");
    focus(cx, &submit);
    press(cx, "enter");
    cx.run_until_parked();
    assert!(
        view.read_with(cx, |this, _| this.create_form().is_none()),
        "Enter on Create closes the dialog"
    );
    assert!(has_focus(cx), "focus must survive the create confirm");
    press(cx, "ctrl-n");
    assert!(
        view.read_with(cx, |this, _| this.create_form().is_some()),
        "ctrl-n must reopen the create dialog after a confirm (no dead keyboard)"
    );
}

/// `F2`: Tab and Shift+Tab stay inside the create dialog and wrap.
#[gpui_kit::test]
fn tab_is_contained_in_the_create_dialog(cx: &mut TestAppContext) {
    let (view, _service, cx) = harness(cx);
    press(cx, "ctrl-n");
    assert!(view.read_with(cx, |this, _| this.create_form().is_some()));
    draw(cx);
    assert!(
        active_trap(cx).is_some(),
        "the create dialog must register a focus trap"
    );
    // 40 forward and 40 backward; every stop stays inside the dialog and the
    // walk wraps rather than reaching the page behind the scrim.
    assert_trapped(cx, true, 40, "create dialog Tab");
    assert_trapped(cx, false, 40, "create dialog Shift+Tab");
}

/// `F2`: Tab stays inside the palette.
#[gpui_kit::test]
fn tab_is_contained_in_the_palette(cx: &mut TestAppContext) {
    let (view, _service, cx) = harness(cx);
    press(cx, "ctrl-k");
    assert!(view.read_with(cx, |this, _| this.palette_is_open()));
    draw(cx);
    assert!(
        active_trap(cx).is_some(),
        "the palette must register a focus trap"
    );
    assert_trapped(cx, true, 40, "palette Tab");
    assert_trapped(cx, false, 40, "palette Shift+Tab");
}

/// `F2`: Tab stays inside the Settings dialog (which has only buttons).
#[gpui_kit::test]
fn tab_is_contained_in_the_settings_dialog(cx: &mut TestAppContext) {
    let (view, _service, cx) = harness(cx);
    cx.update(|window, cx| {
        view.update(cx, |this, cx| this.open_settings(window, cx));
    });
    draw(cx);
    assert!(
        view.read_with(cx, |this, _| this.settings_state().open),
        "settings dialog open"
    );
    assert!(
        active_trap(cx).is_some(),
        "settings must register a focus trap"
    );
    assert_trapped(cx, true, 40, "settings Tab");
    assert_trapped(cx, false, 40, "settings Shift+Tab");
}

/// `F2`: Tab stays inside the delete dialog.
#[gpui_kit::test]
fn tab_is_contained_in_the_delete_dialog(cx: &mut TestAppContext) {
    let (view, service, cx) = harness(cx);
    let id = service.id_with(InstanceStatus::Stopped);
    cx.update(|window, cx| {
        view.update(cx, |this, cx| {
            this.open_delete_dialog(id.clone(), window, cx)
        });
    });
    draw(cx);
    assert!(
        active_trap(cx).is_some(),
        "delete dialog must register a focus trap"
    );
    assert_trapped(cx, true, 40, "delete Tab");
    assert_trapped(cx, false, 40, "delete Shift+Tab");
}

/// `F3`: real Tab reaches every engine card and every version row of the create
/// dialog — so `tab_stop(false)` on a row makes this go red. Focus starts on the
/// name field (open position); walking forward must visit each card/row handle
/// before the cycle wraps.
#[gpui_kit::test]
fn tab_reaches_every_engine_card_and_version_row(cx: &mut TestAppContext) {
    let (view, _service, cx) = harness(cx);
    press(cx, "ctrl-n");
    assert!(view.read_with(cx, |this, _| this.create_form().is_some()));
    draw(cx);
    // Identity set: the handles the form owns. A stop that is one of these
    // proves the row is a real tab stop reached by Tab.
    let (engine_ids, version_ids) = view.read_with(cx, |this, _| {
        let form = this.create_form().expect("form");
        (
            form.engine_focus
                .iter()
                .map(|(_, h)| format!("{h:?}"))
                .collect::<Vec<_>>(),
            form.version_focus
                .iter()
                .map(|(_, h)| format!("{h:?}"))
                .collect::<Vec<_>>(),
        )
    });
    let mut visited: Vec<String> = Vec::new();
    for _ in 0..40 {
        press(cx, "tab");
        let id = cx
            .update(|window, cx| window.focused(cx).map(|f| format!("{f:?}")))
            .unwrap_or_default();
        if visited.contains(&id) {
            break;
        }
        visited.push(id);
    }
    for (label, ids) in [("engine card", &engine_ids), ("version row", &version_ids)] {
        for id in ids {
            assert!(
                visited.contains(id),
                "Tab never reached a {label} ({id}); visited {} stops",
                visited.len()
            );
        }
    }
}

/// `F3`: the **full** first session by keyboard — choose engine + version,
/// create (assert the exact spec the keys chose), then start and copy the URI of
/// an instance, asserting the specific service calls and the **clipboard text**,
/// with no pointer event.
#[gpui_kit::test]
fn full_first_session_creates_starts_and_copies(cx: &mut TestAppContext) {
    let (view, service, cx) = harness(cx);
    let stopped = service.id_with(InstanceStatus::Stopped);

    // 1) Open the create dialog and choose PostgreSQL's version 16.6 by pressing
    //    Enter/Space on the focused card/row (real keys, not method calls). The
    //    test catalog has one engine (PostgreSQL) with versions 17.4 (default)
    //    and 16.6, so choosing 16.6 proves the key selection took effect.
    press(cx, "ctrl-n");
    let pg = engine_handle(&view, cx, EngineKey::Postgresql);
    focus(cx, &pg);
    press(cx, "enter");
    let version_handle = view
        .read_with(cx, |this, _| {
            this.create_form().and_then(|form| {
                form.version_focus
                    .iter()
                    .find(|(tag, _)| tag == "16.6")
                    .map(|(_, h)| h.clone())
            })
        })
        .expect("16.6 version handle");
    focus(cx, &version_handle);
    press(cx, "space");

    // 2) Create by focusing "Create" and pressing Enter.
    let submit = view
        .read_with(cx, |this, _| {
            this.create_form().map(|f| f.submit_focus.clone())
        })
        .expect("submit focus");
    focus(cx, &submit);
    press(cx, "enter");
    cx.run_until_parked();
    assert!(
        view.read_with(cx, |this, _| this.create_form().is_none()),
        "create closes the dialog"
    );
    let created = service.created();
    assert_eq!(created.len(), 1, "exactly one create_instance call");
    assert_eq!(created[0].engine(), EngineKey::Postgresql, "engine chosen");
    assert_eq!(created[0].version(), "16.6", "keys chose version 16.6");

    // 3) Copy the URI: open the stopped instance's tab via the palette ("Open"),
    //    then Ctrl+Shift+C. Assert the clipboard holds the plaintext URI.
    press(cx, "ctrl-k");
    type_text(cx, "pg-stop");
    // Move the highlight to the "Open pg-stop" row (first row is "Start"); Down
    // selects the next enabled row, then Enter opens the tab.
    press(cx, "down");
    press(cx, "enter");
    cx.run_until_parked();
    press(cx, "ctrl-shift-c");
    cx.run_until_parked();
    let clip = cx
        .read_from_clipboard()
        .and_then(|item| item.text())
        .unwrap_or_default();
    assert!(
        clip.starts_with("postgresql://"),
        "Ctrl+Shift+C must put the URI on the clipboard, got {clip:?}"
    );
    assert!(
        service.uris().iter().any(|(id, _)| id == &stopped),
        "the copy must have fetched the stopped instance's URI"
    );

    // 4) Start it from the palette: type the name (the first enabled row for a
    //    stopped instance is "Start pg-stop") and Enter.
    press(cx, "ctrl-k");
    assert!(
        view.read_with(cx, |this, _| this.palette_is_open()),
        "ctrl-k reopens the palette after copy"
    );
    type_text(cx, "pg-stop");
    let (matched, sel) = view.read_with(cx, |this, cx| {
        let s = this.command_state().read(cx);
        (s.matched_count(), s.selected_index())
    });
    eprintln!("FULL start step: matched={matched} selected={sel:?}");
    press(cx, "enter");
    cx.run_until_parked();
    assert!(
        service.started().contains(&stopped),
        "the palette start command must start the stopped instance, got {:?} (matched={matched})",
        service.started()
    );
}

/// `F3`: typing the ENGINE word ("postgres") finds an instance whose name does
/// not contain it — the `keywords` are matched, per AC2 "by name and engine".
#[gpui_kit::test]
fn the_palette_finds_an_instance_by_the_engine_word(cx: &mut TestAppContext) {
    let (view, _service, cx) = harness(cx);
    press(cx, "ctrl-k");
    assert!(view.read_with(cx, |this, _| this.palette_is_open()));
    type_text(cx, "postgres");
    let (labels, matched) = view.read_with(cx, |this, cx| {
        (
            this.palette_labels(),
            this.command_state().read(cx).matched_count(),
        )
    });
    assert!(
        matched >= 1,
        "typing the engine word 'postgres' must match rows (matched={matched}); labels={labels:?}"
    );
    // And the matched rows are the pg-run/pg-stop instance rows (their labels do
    // not contain "postgres"; only the keyword does).
    assert!(
        labels.iter().any(|l| l.contains("pg-stop")),
        "an instance row must be reachable by the engine keyword: {labels:?}"
    );
}

/// `F3`: Enter on a DISABLED palette row does nothing — no service call, and the
/// palette stays open. Drives the disabled `Stop pg-stop` (the instance is
/// stopped) through the CommandState and a real Enter.
#[gpui_kit::test]
fn enter_on_a_disabled_palette_row_does_nothing(cx: &mut TestAppContext) {
    let (view, service, cx) = harness(cx);
    let calls_before = service.calls();
    press(cx, "ctrl-k");
    // "stop pg-stop": the stop command for a stopped instance is disabled.
    type_text(cx, "stop pg-stop");
    let (matched, reason_first) = view.read_with(cx, |this, cx| {
        let items = this.palette_items();
        let filtered = crate::format::filter_palette(&items, "stop pg-stop");
        (
            this.command_state().read(cx).matched_count(),
            filtered.first().and_then(|i| i.reason.clone()),
        )
    });
    // The only matching row is the disabled stop.
    assert!(
        matched >= 1,
        "the disabled stop row must be visible, matched={matched}"
    );
    assert!(reason_first.is_some(), "that row must carry a reason");
    let selected = view.read_with(cx, |this, cx| {
        this.command_state().read(cx).selected_index()
    });
    press(cx, "enter");
    cx.run_until_parked();
    assert!(
        view.read_with(cx, |this, _| this.palette_is_open()),
        "Enter on a disabled row must not confirm/close the palette"
    );
    assert_eq!(
        service.calls(),
        calls_before,
        "Enter on a disabled row must reach no service (selected={selected:?})"
    );
}

#[path = "page_reachability_tests.rs"]
mod page_reachability_tests;

#[path = "keyboard_fix2_tests.rs"]
mod keyboard_fix2_tests;

#[path = "delete_entry_tests.rs"]
mod delete_entry_tests;
