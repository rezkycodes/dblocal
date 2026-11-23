//! Actions and their key bindings for the workspace.

use gpui_kit::{App, KeyBinding};

/// Root view identity, used as the key-binding context.
pub const ROOT_CONTEXT: &str = "DblocalRoot";

gpui_kit::actions!(
    dblocal_ui,
    [
        /// Toggle the command palette (`Ctrl/Cmd+K`, `Ctrl/Cmd+Shift+P`).
        OpenPalette,
        /// Open the create-instance dialog (`Ctrl/Cmd+N`).
        NewInstance,
        /// Close the top overlay (palette, dialog) with Escape.
        CloseOverlay,
        /// Copy the selected instance's URI (`Ctrl/Cmd+Shift+C`).
        CopyUri,
        /// Open the Settings dialog (`Ctrl/Cmd+,`).
        OpenSettings,
        /// Start or stop the selected instance (`Ctrl/Cmd+Enter`).
        ToggleRun,
        /// Start the selected instance.
        StartSelected,
        /// Stop the selected instance.
        StopSelected,
    ]
);

/// Registers every root-context key binding.
///
/// `F-PLH-01`: the first session is completable from the keyboard, so every
/// command the prototype advertises also has a binding here.
pub fn bind_keys(cx: &mut App) {
    #[cfg(target_os = "macos")]
    let modifiers = [
        KeyBinding::new("cmd-k", OpenPalette, Some(ROOT_CONTEXT)),
        KeyBinding::new("cmd-shift-p", OpenPalette, Some(ROOT_CONTEXT)),
        KeyBinding::new("cmd-n", NewInstance, Some(ROOT_CONTEXT)),
        KeyBinding::new("cmd-shift-c", CopyUri, Some(ROOT_CONTEXT)),
        KeyBinding::new("cmd-,", OpenSettings, Some(ROOT_CONTEXT)),
        KeyBinding::new("cmd-enter", ToggleRun, Some(ROOT_CONTEXT)),
    ];
    #[cfg(not(target_os = "macos"))]
    let modifiers = [
        KeyBinding::new("ctrl-k", OpenPalette, Some(ROOT_CONTEXT)),
        KeyBinding::new("ctrl-shift-p", OpenPalette, Some(ROOT_CONTEXT)),
        KeyBinding::new("ctrl-n", NewInstance, Some(ROOT_CONTEXT)),
        KeyBinding::new("ctrl-shift-c", CopyUri, Some(ROOT_CONTEXT)),
        KeyBinding::new("ctrl-,", OpenSettings, Some(ROOT_CONTEXT)),
        KeyBinding::new("ctrl-enter", ToggleRun, Some(ROOT_CONTEXT)),
    ];
    cx.bind_keys(modifiers);
    cx.bind_keys([
        KeyBinding::new("escape", CloseOverlay, Some(ROOT_CONTEXT)),
        KeyBinding::new("ctrl-shift-s", StartSelected, Some(ROOT_CONTEXT)),
        KeyBinding::new("ctrl-shift-x", StopSelected, Some(ROOT_CONTEXT)),
    ]);
}

/// Returns the platform modifier name shown in hints (`Ctrl` or `Cmd`).
pub fn modifier_label() -> &'static str {
    if cfg!(target_os = "macos") {
        "Cmd"
    } else {
        "Ctrl"
    }
}

/// The palette hint rendered in the title bar, e.g. `Ctrl K`.
pub fn palette_hint() -> String {
    format!("{} K", modifier_label())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hint_matches_platform_convention() {
        let label = modifier_label();
        assert!(label == "Cmd" || label == "Ctrl");
        assert!(palette_hint().ends_with(" K"));
    }
}
