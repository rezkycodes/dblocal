//! In-app update strings (`F-PKG-05`).
//!
//! The title-bar download button, the update dialog's stages, and the Settings
//! section's automatic-check switch. One term per concept: the action is always
//! "update", the running release is a "version".

/// Title-bar update button tooltip; `{version}` becomes the newer release.
pub const UI_UPDATE_TOOLTIP: &str = "Update to version {version}";
/// Update dialog title.
pub const UI_UPDATE_DIALOG_TITLE: &str = "DBLocal update";
/// Update dialog: the asset is downloading.
pub const UI_UPDATE_DOWNLOADING: &str = "Downloading version {version}…";
/// Update dialog: the checksum is being verified.
pub const UI_UPDATE_VERIFYING: &str = "Verifying the download…";
/// Update dialog: the files are being installed.
pub const UI_UPDATE_INSTALLING: &str = "Installing version {version}…";
/// Update dialog: installed, restarting into the new version.
pub const UI_UPDATE_READY: &str = "Version {version} is installed. Restarting DBLocal…";
/// Update dialog: restart into the installed version now.
pub const UI_UPDATE_READY_RESTART: &str = "Restart now";
/// Update dialog: a download/install is in flight.
pub const UI_UPDATE_IN_PROGRESS: &str = "This can take a minute.";
/// Update dialog: the update stopped before it finished.
pub const UI_UPDATE_FAILED: &str = "The update did not finish.";
/// Update dialog: the user cancelled while downloading.
pub const UI_UPDATE_CANCELLED: &str = "Update cancelled. DBLocal is unchanged.";
/// Update dialog: this install method cannot replace its own binary.
pub const UI_UPDATE_CANNOT: &str = "This install cannot update itself.";
/// Update dialog: what to do instead when self-update is refused.
pub const UI_UPDATE_CANNOT_BODY: &str = "Download the matching package from the release page.";
/// Update dialog: cancel action while downloading.
pub const UI_UPDATE_CANCEL: &str = "Cancel update";
/// Update dialog: retry a failed or cancelled update.
pub const UI_UPDATE_RETRY: &str = "Try again";
/// Update dialog and Settings: open the GitHub release page.
pub const UI_UPDATE_RELEASE_PAGE: &str = "Open release page";
/// Update dialog: dismiss action.
pub const UI_UPDATE_CLOSE: &str = "Close";
/// Settings updates: the manual check action, beside the automatic switch.
pub const UI_SETTINGS_CHECK_UPDATES: &str = "Check for updates";
/// Settings updates: the automatic-check switch label.
pub const UI_SETTINGS_AUTO_CHECK: &str = "Check for updates automatically";
/// Settings updates: what the automatic check does.
pub const UI_SETTINGS_AUTO_CHECK_BODY: &str =
    "At startup and about every 6 hours. Nothing is installed until you press the download icon.";

/// Formats the title-bar tooltip for a newer version.
pub fn update_tooltip(version: &str) -> String {
    UI_UPDATE_TOOLTIP.replace("{version}", version)
}

/// Formats the downloading headline for a newer version.
pub fn update_downloading(version: &str) -> String {
    UI_UPDATE_DOWNLOADING.replace("{version}", version)
}

/// Formats the installing headline for a newer version.
pub fn update_installing(version: &str) -> String {
    UI_UPDATE_INSTALLING.replace("{version}", version)
}

/// Formats the "restarting into the new version" headline.
pub fn update_ready(version: &str) -> String {
    UI_UPDATE_READY.replace("{version}", version)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_template_substitutes_its_version() {
        assert_eq!(update_tooltip("0.9.0"), "Update to version 0.9.0");
        assert_eq!(update_downloading("0.9.0"), "Downloading version 0.9.0…");
        assert_eq!(update_installing("0.9.0"), "Installing version 0.9.0…");
        assert_eq!(
            update_ready("0.9.0"),
            "Version 0.9.0 is installed. Restarting DBLocal…"
        );
        for text in [
            update_tooltip("1.0.0"),
            update_downloading("1.0.0"),
            update_installing("1.0.0"),
            update_ready("1.0.0"),
        ] {
            assert!(!text.contains("{version}"), "unsubstituted: {text}");
        }
    }

    #[test]
    fn sentences_are_sentence_case_without_trailing_periods_in_labels() {
        for label in [
            UI_UPDATE_DIALOG_TITLE,
            UI_UPDATE_CANCEL,
            UI_UPDATE_RETRY,
            UI_UPDATE_RELEASE_PAGE,
            UI_UPDATE_CLOSE,
            UI_SETTINGS_AUTO_CHECK,
        ] {
            assert!(!label.is_empty());
            assert!(
                !label.ends_with('.'),
                "a label must not end in a period: {label}"
            );
        }
    }
}
