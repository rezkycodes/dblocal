//! `F-INS-05` entry points, driven by real keys (W4-I D1).
//!
//! The delete dialog, its typed-name check and `confirm_delete` already existed
//! and were unit-tested, but nothing in the real app could open it: the only
//! delete was `dblocal delete` in the CLI. These tests prove the two entry
//! points the GUI now has — the instance header's "Delete" button and the palette
//! row "Delete <name>" — reach the same handler, and that the mode and instance
//! the user chose are what the service is asked to delete.
//!
//! Included as a child of the `keyboard_flow_tests` module so it shares the
//! harness and the real-keystroke helpers. Every mutation that turns one of
//! these red is in `docs/dev/accessibility-checklist.md`.

use super::*;
use dblocal_core::model::DeleteMode;

/// The dialog open for `id`, with its mode.
fn dialog_for(view: &Entity<Workspace>, cx: &VisualTestContext) -> Option<String> {
    view.read_with(cx, |this, cx| this.delete_target_name(cx))
}

/// `F-INS-05`: Tab reaches the header's "Delete" button and Enter opens the
/// dialog — the real control, not a direct method call.
#[gpui_kit::test]
fn the_header_delete_button_opens_the_dialog(cx: &mut TestAppContext) {
    let (view, service, cx) = harness(cx);
    // Put an instance on screen so the header (and its buttons) exists; the
    // header is drawn only for the selected tab.
    let id = service.id_with(InstanceStatus::Stopped);
    cx.update(|_window, cx| {
        view.update(cx, |this, cx| this.open_tab(id.clone(), cx));
    });
    cx.run_until_parked();
    draw(cx);

    // Walk Tab until the delete button's own handle holds focus, then Enter.
    // This is the exact path a keyboard user takes; the handle identity proves
    // it was the button, not a coincidence of stop order.
    let delete = view.read_with(cx, |this, _| this.delete_button_focus());
    let mut reached = false;
    for _ in 0..60 {
        press(cx, "tab");
        if cx.update(|window, _cx| delete.is_focused(window)) {
            reached = true;
            break;
        }
    }
    assert!(
        reached,
        "Tab must reach the header Delete button (F-INS-05 entry point)"
    );
    press(cx, "enter");
    cx.run_until_parked();
    draw(cx);
    assert_eq!(
        dialog_for(&view, cx),
        Some("pg-stop".to_owned()),
        "Enter on the header Delete button must open the delete dialog for its instance"
    );
}

/// `F-INS-05`: the palette row "Delete <name>" opens the dialog for **that**
/// instance, not the first row — driven by typing a query that leaves a
/// non-first instance's row and pressing Enter.
#[gpui_kit::test]
fn the_palette_delete_row_opens_the_dialog_for_that_instance(cx: &mut TestAppContext) {
    let (view, service, cx) = harness(cx);
    // pg-stop is the second instance; typing its name leaves "Delete pg-stop"
    // as the only delete row, so the confirm cannot be the first instance's.
    let stopped = service.id_with(InstanceStatus::Stopped);
    press(cx, "ctrl-k");
    assert!(view.read_with(cx, |this, _| this.palette_is_open()));
    type_text(cx, "delete pg-stop");
    let (matched, first_label) = view.read_with(cx, |this, cx| {
        let items = this.palette_items();
        let filtered = crate::format::filter_palette(&items, "delete pg-stop");
        (
            this.command_state().read(cx).matched_count(),
            filtered
                .first()
                .map(|i| i.label.clone())
                .unwrap_or_default(),
        )
    });
    assert_eq!(
        matched, 1,
        "the query must leave exactly the delete row for pg-stop (got {matched}: {first_label})"
    );
    press(cx, "enter");
    cx.run_until_parked();
    draw(cx);
    assert!(
        !view.read_with(cx, |this, _| this.palette_is_open()),
        "the palette closes when its row opens the dialog"
    );
    assert_eq!(
        dialog_for(&view, cx),
        Some("pg-stop".to_owned()),
        "the palette row must open the dialog for the instance it names"
    );
    assert_eq!(
        view.read_with(cx, |this, _| this.delete_mode()),
        DeleteMode::ArchiveData,
        "the dialog starts in archive mode"
    );
    assert_eq!(
        view.read_with(cx, |this, _| this.delete_target_id()),
        Some(stopped),
        "the dialog targets the palette row's instance"
    );
}

/// `F-INS-05` AC3: archive mode confirms **without** a typed name and the
/// service gets `DeleteMode::ArchiveData` for that id. After the confirm the row
/// leaves the sidebar and the keyboard is alive (`ctrl-k` opens).
#[gpui_kit::test]
fn archive_mode_confirms_without_a_name_and_asks_for_archive(cx: &mut TestAppContext) {
    let (view, service, cx) = harness(cx);
    let stopped = service.id_with(InstanceStatus::Stopped);

    // Open through the palette row, then confirm with Enter on the dialog's
    // default "Delete" button — no name typed.
    press(cx, "ctrl-k");
    type_text(cx, "delete pg-stop");
    press(cx, "enter");
    cx.run_until_parked();
    draw(cx);
    assert_eq!(dialog_for(&view, cx), Some("pg-stop".to_owned()));

    cx.update(|_window, cx| {
        view.update(cx, |this, cx| this.confirm_delete_action(cx));
    });
    cx.run_until_parked();
    draw(cx);

    assert_eq!(
        service.deleted(),
        vec![(stopped.clone(), DeleteMode::ArchiveData)],
        "archive mode must ask the service for ArchiveData for the opened instance"
    );
    assert!(
        view.read_with(cx, |this, _| this.delete_target.is_none()),
        "the confirm closes the dialog"
    );
    // The row leaves the sidebar: the service no longer lists it and the
    // refresh that follows a delete applies that read.
    assert!(
        !view.read_with(cx, |this, _| this
            .instances()
            .iter()
            .any(|v| v.id == stopped)),
        "the deleted instance must leave the sidebar after the refresh"
    );
    // `F1`: the keyboard is alive right after the confirm.
    assert!(has_focus(cx), "focus must survive the delete confirm");
    press(cx, "ctrl-k");
    assert!(
        view.read_with(cx, |this, _| this.palette_is_open()),
        "ctrl-k must open right after a delete confirm (no dead keyboard)"
    );
}

/// `F-INS-05` AC2: purge mode needs the exact typed name. A wrong name leaves
/// the dialog open and reaches no service; the exact name enables the confirm.
#[gpui_kit::test]
fn purge_mode_needs_the_typed_name(cx: &mut TestAppContext) {
    let (view, service, cx) = harness(cx);
    let running = service.id_with(InstanceStatus::Running);

    cx.update(|window, cx| {
        view.update(cx, |this, cx| {
            this.open_delete_dialog(running.clone(), window, cx)
        });
    });
    cx.run_until_parked();
    draw(cx);
    view.update(cx, |this, _| this.set_delete_mode(DeleteMode::PurgeData));
    draw(cx);

    // A wrong name: the check is false, and a confirm is a no-op. The dialog's
    // field entity is the source of truth while it exists, so type into it
    // exactly as the user does.
    cx.update(|window, cx| {
        view.update(cx, |this, cx| {
            if let Some(input) = this.delete_name_input.clone() {
                input.update(cx, |state, cx| {
                    state.set_value("salah".to_owned(), window, cx)
                });
            }
        });
    });
    assert!(
        !view.read_with(cx, |this, cx| this.delete_confirmation_ok(cx)),
        "a wrong name must not satisfy the purge check"
    );
    let calls_before = service.calls();
    cx.update(|_window, cx| {
        view.update(cx, |this, cx| this.confirm_delete_action(cx));
    });
    cx.run_until_parked();
    assert_eq!(
        service.calls(),
        calls_before,
        "a wrong name must reach no service"
    );
    assert!(
        view.read_with(cx, |this, _| this.delete_target.is_some()),
        "a wrong name leaves the dialog open"
    );

    // The exact name enables the confirm.
    cx.update(|window, cx| {
        view.update(cx, |this, cx| {
            if let Some(input) = this.delete_name_input.clone() {
                input.update(cx, |state, cx| {
                    state.set_value("pg-run".to_owned(), window, cx)
                });
            }
        });
    });
    assert!(
        view.read_with(cx, |this, cx| this.delete_confirmation_ok(cx)),
        "the exact name must satisfy the purge check"
    );
    cx.update(|_window, cx| {
        view.update(cx, |this, cx| this.confirm_delete_action(cx));
    });
    cx.run_until_parked();
    assert_eq!(
        service.deleted(),
        vec![(running, DeleteMode::PurgeData)],
        "the exact name in purge mode must ask the service for PurgeData"
    );
}

/// `F2` + `F-INS-05` AC2: the two mode cards are real tab stops, so a
/// keyboard-only user can reach purge mode and turn it on with Enter. Before this
/// the cards listened only for the mouse, so "Delete instance and data" was
/// unreachable without a pointer.
#[gpui_kit::test]
fn tab_reaches_the_purge_mode_card_and_enter_selects_it(cx: &mut TestAppContext) {
    let (view, service, cx) = harness(cx);
    let running = service.id_with(InstanceStatus::Running);

    cx.update(|window, cx| {
        view.update(cx, |this, cx| {
            this.open_delete_dialog(running.clone(), window, cx)
        });
    });
    cx.run_until_parked();
    draw(cx);
    assert_eq!(
        view.read_with(cx, |this, _| this.delete_mode()),
        DeleteMode::ArchiveData,
        "the dialog opens in archive mode"
    );

    // Tab from the dialog container: archive card, purge card, Cancel, Delete, wrap.
    // Walk until the purge card holds focus, then Enter selects it. The mode is
    // read from the workspace, not a direct setter call, so only a real control
    // activation can flip it.
    let [_archive, purge] = view.read_with(cx, |this, _| this.delete_mode_focus());
    let mut reached_purge = false;
    for _ in 0..10 {
        press(cx, "tab");
        if cx.update(|window, _cx| purge.is_focused(window)) {
            reached_purge = true;
            break;
        }
    }
    assert!(
        reached_purge,
        "Tab must reach the purge mode card (F-INS-05 AC2: purge needs no pointer)"
    );
    press(cx, "enter");
    cx.run_until_parked();
    draw(cx);
    assert_eq!(
        view.read_with(cx, |this, _| this.delete_mode()),
        DeleteMode::PurgeData,
        "Enter on the focused purge card must switch the dialog to purge mode"
    );
}

/// `F2`: Tab stays inside the delete dialog opened from the header, and Escape
/// returns focus to the button that opened it.
#[gpui_kit::test]
fn the_delete_dialog_from_the_header_traps_tab_and_returns_focus(cx: &mut TestAppContext) {
    let (view, service, cx) = harness(cx);
    let id = service.id_with(InstanceStatus::Stopped);
    cx.update(|_window, cx| {
        view.update(cx, |this, cx| this.open_tab(id.clone(), cx));
    });
    cx.run_until_parked();
    draw(cx);

    // Focus the header button and open the dialog with Enter, so the opener is
    // exactly this control and Escape must give the keyboard back to it.
    let delete = view.read_with(cx, |this, _| this.delete_button_focus());
    focus(cx, &delete);
    press(cx, "enter");
    cx.run_until_parked();
    draw(cx);
    assert_eq!(dialog_for(&view, cx), Some("pg-stop".to_owned()));
    assert!(
        active_trap(cx).is_some(),
        "the delete dialog must register a focus trap"
    );
    assert_trapped(cx, true, 40, "delete Tab");
    assert_trapped(cx, false, 40, "delete Shift+Tab");

    press(cx, "escape");
    cx.run_until_parked();
    draw(cx);
    assert!(
        view.read_with(cx, |this, _| this.delete_target.is_none()),
        "Escape closes the delete dialog"
    );
    assert!(
        cx.update(|window, _cx| delete.is_focused(window)),
        "Escape must return focus to the header Delete button that opened the dialog"
    );
}
