//! Keyboard focus for the Databases dialogs (`F-PLH-01`, F3).
//!
//! A dialog that opens while the covered field still holds focus swallows every
//! keystroke into that field: the reviewer typed `x` into the drop dialog and it
//! landed in the create-database field behind the overlay, and Enter confirmed
//! nothing because the dialog's own field had never been focused.
//!
//! Two moves fix it: the dialog takes focus when it opens, and the handle that
//! had it gets focus back when the dialog closes. The ordering — capture the
//! previous handle once, restore exactly that, never return to a dialog that is
//! itself closing — lives here so it is unit-tested without a window.

use gpui_kit::FocusHandle;

/// The dialog focus handoff: who had focus, and who takes it back.
#[derive(Debug, Default)]
pub struct DialogFocus {
    /// The handle that had focus when a dialog opened, while one is open.
    previous: Option<FocusHandle>,
}

impl DialogFocus {
    /// Records that a dialog is opening, remembering what had focus.
    ///
    /// A second dialog opening on top of the first must not overwrite the
    /// capture: returning to the first dialog would strand the user there once
    /// that one closes too. A `None` capture (nothing focused, or a headless
    /// preview) is remembered as "nothing", not retried on every later open.
    pub fn opened(&mut self, previous: Option<FocusHandle>) {
        if self.previous.is_none() {
            self.previous = previous;
        }
    }

    /// Takes the handle to focus back when the last dialog closes.
    ///
    /// `None` when no dialog was open or nothing had focus, so every close path
    /// can call it unconditionally.
    pub fn closed(&mut self) -> Option<FocusHandle> {
        self.previous.take()
    }

    /// The handle a close would restore, for assertions.
    #[cfg(test)]
    pub fn return_target(&self) -> Option<&FocusHandle> {
        self.previous.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::TestAppContext;

    #[gpui_kit::test]
    fn the_first_open_captures_and_a_second_open_does_not_overwrite(cx: &mut TestAppContext) {
        let (field, first) = cx.update(|cx| (cx.focus_handle(), cx.focus_handle()));
        let mut focus = DialogFocus::default();

        focus.opened(Some(field.clone()));
        focus.opened(Some(first));
        assert_eq!(
            focus.return_target(),
            Some(&field),
            "a dialog stacked on another must still return to the field"
        );

        assert_eq!(focus.closed(), Some(field.clone()));
        assert_eq!(focus.closed(), None, "a second close restores nothing");
    }

    #[gpui_kit::test]
    fn a_close_without_an_open_is_a_no_op() {
        let mut focus = DialogFocus::default();
        assert!(focus.closed().is_none());
        assert!(focus.return_target().is_none());
    }

    #[gpui_kit::test]
    fn an_open_with_nothing_focused_restores_nothing() {
        let mut focus = DialogFocus::default();
        focus.opened(None);
        assert!(focus.closed().is_none());
    }
}
