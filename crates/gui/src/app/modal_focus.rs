//! Modal focus lifecycle: dismissal, the opener/root focus return, and the
//! per-modal focus-trap container (`F-PLH-01`, `F1`, `F2`).
//!
//! Split out of `app.rs` to keep it under the 800-line limit. Every overlay close
//! funnels through `restore_dialog_focus_after_close` so a path cannot forget.

use gpui_kit::{Context, FocusHandle, Window};

use super::Workspace;

impl Workspace {
    pub(crate) fn close_modals(&mut self) {
        if self.palette_open {
            // The palette owns its focus capture/restore (F-PLH-01); closing it
            // through here must return focus too, not just flip the flag.
            // `close_palette` needs no window: it parks the restore for the
            // next frame.
            self.palette_open = false;
            self.restore_dialog_focus_after_close();
            return;
        }
        // Escape on the create or delete dialog returns focus to the opener.
        if self.create.take().is_some() {
            self.restore_dialog_focus_after_close();
            return;
        }
        if self.delete_target.take().is_some() {
            self.delete_name_input = None;
            self.restore_dialog_focus_after_close();
            return;
        }
        self.delete_name_input = None;
        // The top overlay only: Escape closes the topmost dialog, so a secret
        // dialog closes first, then the confirmation under it stays open.
        if self.secret_dialog.take().is_some() {
            self.restore_dialog_focus_after_close();
            return;
        }
        if self.drop_dialog.take().is_some() {
            self.restore_dialog_focus_after_close();
            return;
        }
        if self.reset_confirm.take().is_some() {
            self.restore_dialog_focus_after_close();
            return;
        }
        if self.config_restore_confirm.take().is_some() {
            self.restore_dialog_focus_after_close();
            return;
        }
        // The update dialog is drawn after Settings, so it is the top overlay
        // when both are open: Escape closes it and Settings stays.
        if self.update.dialog_visible() {
            self.update.dismiss_dialog();
            self.restore_dialog_focus_after_close();
            return;
        }
        if self.settings.open {
            self.settings.close();
            self.restore_dialog_focus_after_close();
        }
    }

    /// Whether any overlay is on screen, so a caller can decide whether a
    /// dismissal should return focus (an action that opened a new overlay owns
    /// its own capture; one that ran a command and opened nothing must restore).
    pub(crate) fn any_overlay_open(&self) -> bool {
        self.palette_open
            || self.create.is_some()
            || self.delete_target.is_some()
            || self.settings.open
            || self.update.dialog_visible()
            || self.drop_dialog.is_some()
            || self.reset_confirm.is_some()
            || self.secret_dialog.is_some()
            || self.config_restore_confirm.is_some()
            || self.quit.is_confirming()
    }

    /// The single place every overlay dismissal returns the keyboard.
    ///
    /// `F1`: when an overlay closes — by Escape, by confirming, by a button, or
    /// because the action finished — focus must land back on something. The
    /// opener's handle is restored when it was captured and still exists;
    /// otherwise (the overlay was opened with nothing focused, as at startup, or
    /// the opener was removed) focus falls back to the Workspace root, so the
    /// keyboard never ends with `window.focused() == None` (a dead keyboard: the
    /// real window needed a mouse click before `ctrl-k` worked again).
    ///
    /// Runs from listeners that may have no window (Escape, a background
    /// confirmation), so it parks the handle for `render` to apply on the next
    /// frame rather than focusing here.
    pub(crate) fn restore_dialog_focus_after_close(&mut self) {
        let target = self
            .dialog_focus
            .closed()
            .unwrap_or_else(|| self.focus_handle.clone());
        self.pending_focus_restore = Some(target);
        // No modal is open now; the trap handle is stale.
        self.modal_focus = None;
    }

    /// Closes the create dialog after a successful submit and returns focus.
    ///
    /// `submit_create` takes `self.create` before the async submit, so the
    /// success arm has to restore focus itself; a failed submit re-opens the
    /// form and keeps focus in it, so it does not.
    pub(crate) fn finish_create_overlay(&mut self) {
        self.restore_dialog_focus_after_close();
    }

    /// Opens a modal: captures the opener for the focus return (`F1`) and makes a
    /// fresh container handle that its render traps Tab inside (`F2`).
    ///
    /// The opener is captured here, once, through `dialog_focus::opened`, which
    /// ignores a stacked open so the original opener is the one restored.
    pub(crate) fn begin_modal(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> FocusHandle {
        self.dialog_focus.opened(window.focused(cx));
        let handle = cx.focus_handle();
        self.modal_focus = Some(handle.clone());
        handle
    }

    /// Opens a modal from a background result, where no window is in hand.
    ///
    /// Same effect as [`Self::begin_modal`] minus the opener capture: there was
    /// no focused element at startup, so the close path falls back to the root
    /// handle (see `restore_dialog_focus_after_close`).
    pub(crate) fn begin_modal_without_window(&mut self, cx: &mut Context<Self>) {
        self.dialog_focus.opened(None);
        self.modal_focus = Some(cx.focus_handle());
        self.focus_modal_root();
    }

    /// Gives the keyboard to an element inside a just-opened modal on the next
    /// frame (it does not exist in the focus tree until it is drawn once).
    pub(crate) fn focus_modal_root(&mut self) {
        if let Some(handle) = self.modal_focus.clone() {
            self.pending_focus_restore = Some(handle);
        }
    }

    /// The container focus handle of the open modal, for its render and tests.
    pub(crate) fn modal_focus(&self) -> Option<FocusHandle> {
        self.modal_focus.clone()
    }
}
