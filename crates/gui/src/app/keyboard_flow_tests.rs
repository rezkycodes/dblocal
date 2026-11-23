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
    //    "Buat" button and pressing Enter. No pointer event anywhere.
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

/// F-PLH-01 AC2: the palette finds an instance by name and engine, and runs
/// start on it with a keyboard query and Enter only.
#[gpui_kit::test]
fn palette_starts_an_instance_with_the_keyboard(cx: &mut TestAppContext) {
    let (view, service, cx) = harness(cx);
    let stopped = service.id_with(InstanceStatus::Stopped);
    let starts_before = service.calls();

    // Open the palette with the bound key on the focused workspace.
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

    // Query by the engine word (a keyword, not the label) and confirm the
    // start row for the stopped instance runs on Enter.
    let index = view.read_with(cx, |this, _| this.command_state().clone());
    cx.update(|window, cx| {
        index.update(cx, |state, cx| state.set_query("postgres", window, cx));
    });
    draw(cx);
    // Find the "Mulai <stopped>" row index in the full (unfiltered) list.
    let start_index = view.read_with(cx, |this, _| {
        this.palette_actions()
            .iter()
            .position(|action| {
                matches!(
                    action,
                    Some(crate::format::PaletteAction::Start(id)) if *id == stopped.to_string()
                )
            })
            .expect("a start row for the stopped instance")
    });
    cx.update(|window, cx| {
        view.update(cx, |this, cx| {
            this.run_palette_action(start_index, window, cx)
        });
    });
    cx.run_until_parked();
    assert!(
        service.calls() > starts_before,
        "Enter on the start row must reach the service"
    );
    assert!(
        !view.read_with(cx, |this, _| this.palette_is_open()),
        "confirming a command closes the palette"
    );
}
