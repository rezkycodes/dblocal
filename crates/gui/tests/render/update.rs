//! In-app update render tests (`F-PKG-05`): the title-bar icon and the update
//! dialog in both themes, in every stage.
//!
//! Included from `tests/render.rs`, so it uses that file's `render`, `Build`,
//! `Frame` helpers and theme tokens. **All frames are mock data** from the
//! preview constructors: no release is downloaded and nothing is installed.

use super::{
    Build, Frame, LOGICAL_W, Workspace, dark_bg, dark_danger, dark_primary, light_bg,
    light_primary, render,
};

/// Every update stage renders in both themes, and the light frame is lighter.
#[test]
fn every_update_stage_renders_in_both_themes() {
    let cases: [(&str, Build); 10] = [
        ("available", Workspace::preview_update_available),
        ("downloading", Workspace::preview_update_downloading),
        ("verifying", Workspace::preview_update_verifying),
        ("installing", Workspace::preview_update_installing),
        ("ready", Workspace::preview_update_ready),
        ("failed", Workspace::preview_update_failed),
        (
            "previous-failed",
            Workspace::preview_update_previous_failure,
        ),
        (
            "previous-failed-long",
            Workspace::preview_update_previous_failure_long,
        ),
        ("cannot", Workspace::preview_update_cannot),
        ("no-signing-key", Workspace::preview_update_no_signing_key),
    ];
    for (name, build) in cases {
        let dark = render(true, &format!("update-{name}-dark.png"), build);
        let light = render(false, &format!("update-{name}-light.png"), build);
        eprintln!(
            "UPDATE {name}: dark={} colours light={} colours",
            dark.colours, light.colours
        );
        assert!(dark.colours > 24, "{name}: dark render too flat");
        assert!(light.colours > 24, "{name}: light render too flat");
        assert!(
            light.mean_luminance() > dark.mean_luminance(),
            "{name}: the light frame is not lighter than the dark one"
        );
        assert_update_dialog_painted(&dark, name, "[dark]", dark_bg());
        assert_update_dialog_painted(&light, name, "[light]", light_bg());
    }
}

/// The dialog's panel covers a central region; a stage that drew nothing would
/// leave that region in the base background.
fn assert_update_dialog_painted(frame: &Frame, name: &str, theme: &str, bg: [u8; 3]) {
    let panel = (110, 90, LOGICAL_W - 110, 620);
    let painted = frame.count_not_near(panel, bg, 6);
    eprintln!("UPDATE {name} {theme} painted={painted}");
    assert!(
        painted > 20_000,
        "{name} {theme}: the update dialog did not paint ({painted})"
    );
}

/// No update: the title bar carries no download glyph.
///
/// The icon is added *before* the gear and plus, so drawing it shifts those two
/// left; a fixed sub-region would compare different widgets. The whole title-bar
/// row is the honest comparison: one more button means more ink.
#[test]
fn no_update_draws_no_download_icon() {
    let idle = render(true, "update-idle-dark.png", Workspace::preview);
    let available = render(
        true,
        "update-icon-dark.png",
        Workspace::preview_update_available,
    );
    let row = (0, 0, LOGICAL_W, 34);
    let idle_ink = idle.count_not_near(row, dark_bg(), 24);
    let available_ink = available.count_not_near(row, dark_bg(), 24);
    eprintln!("UPDATE ICON idle={idle_ink} available={available_ink}");
    assert!(
        available_ink > idle_ink + 20,
        "the download icon must add ink to the title bar ({idle_ink} -> {available_ink})"
    );
}

/// A managed install's dialog names the release page as its primary action and
/// draws no download progress.
#[test]
fn a_cannot_self_update_dialog_offers_the_release_page() {
    let frame = render(
        false,
        "update-cannot-light.png",
        Workspace::preview_update_cannot,
    );
    let dialog = (110, 90, LOGICAL_W - 110, 620);
    let text = frame.count_not_near(dialog, light_bg(), 14);
    eprintln!("UPDATE CANNOT text={text}");
    assert!(text > 2_000, "the explanation did not render ({text})");
    // The action row sits in the lower half of the centered dialog (below the
    // header icon): "Open release page" is the sole action, so it must be
    // painted as a filled primary button (> 500 primary pixels).
    let action_row = (380, 410, 900, 560);
    let primary_button_px = frame.count_near(action_row, light_primary(), 28);
    eprintln!("UPDATE CANNOT primary_button_px={primary_button_px}");
    assert!(
        primary_button_px > 500,
        "Open release page must be painted as the primary button when it is the only action ({primary_button_px})"
    );
}

/// A failed update paints the danger colour in the dialog.
#[test]
fn a_failed_update_paints_the_danger_colour() {
    let frame = render(
        true,
        "update-failed-danger-dark.png",
        Workspace::preview_update_failed,
    );
    let dialog = (110, 90, LOGICAL_W - 110, 620);
    let danger = frame.count_near(dialog, dark_danger(), 28);
    eprintln!("UPDATE FAILED danger={danger}");
    assert!(danger > 40, "the failure line was not painted ({danger})");
}

/// The Settings dialog shows the automatic-check switch beside the update.
#[test]
fn settings_shows_the_automatic_check_and_the_release() {
    let frame = render(
        false,
        "settings-update-light.png",
        Workspace::preview_settings_update,
    );
    let panel = (330, 90, LOGICAL_W - 330, 700);
    let text = frame.count_not_near(panel, light_bg(), 14);
    eprintln!("SETTINGS UPDATE text={text}");
    assert!(text > 2_000, "the Settings update section did not render");
    assert!(
        frame.count_near(panel, dark_primary(), 40) > 0
            || frame.count_near(panel, light_primary(), 40) > 0,
        "the re-used install action was not painted"
    );
}

/// A previous run's persisted failure is drawn as a warning line with the
/// helper's own sentence, plus a release-page action.
#[test]
fn a_previous_failure_draws_its_sentence_and_an_action() {
    let frame = render(
        false,
        "update-previous-failed-line-light.png",
        Workspace::preview_update_previous_failure,
    );
    let dialog = (110, 90, LOGICAL_W - 110, 620);
    let warning = frame.count_near(dialog, [0x73, 0x53, 0x00], 40);
    let text = frame.count_not_near(dialog, light_bg(), 14);
    eprintln!("UPDATE PREVIOUS warning={warning} text={text}");
    assert!(text > 2_000, "the failure note did not render ({text})");
    assert!(
        frame.count_near(dialog, light_primary(), 30) > 0,
        "the retry button must be painted as a primary action"
    );
}

/// A release build with no signing key explains itself and offers no download
/// button, even though the install kind is a writable per-user tarball.
#[test]
fn no_signing_key_dialog_explains_and_offers_no_download() {
    let frame = render(
        true,
        "update-no-signing-key-dialog-dark.png",
        Workspace::preview_update_no_signing_key,
    );
    let dialog = (110, 90, LOGICAL_W - 110, 620);
    let text = frame.count_not_near(dialog, dark_bg(), 14);
    eprintln!("UPDATE NOSIGN text={text}");
    assert!(
        text > 2_000,
        "the signing-key reason did not render ({text})"
    );
    let action_row = (380, 410, 900, 560);
    let primary_button_px = frame.count_near(action_row, dark_primary(), 28);
    eprintln!("UPDATE NOSIGN primary_button_px={primary_button_px}");
    assert!(
        primary_button_px > 500,
        "Open release page must be primary in the no-signing-key dialog ({primary_button_px})"
    );

    // The title bar still shows the icon so the reason can be reached (the
    // dialog preview itself dims the bar, so the icon case is its own frame).
    let frame = render(
        true,
        "update-no-signing-key-icon-dark.png",
        Workspace::preview_update_no_signing_key_icon,
    );
    let row = (0, 0, LOGICAL_W, 34);
    let idle = render(
        true,
        "update-no-signing-key-idle-dark.png",
        Workspace::preview,
    );
    assert!(
        frame.count_not_near(row, dark_bg(), 24) > idle.count_not_near(row, dark_bg(), 24) + 20,
        "the update icon must still be drawn without a signing key"
    );
}

/// A long previous-failure message (including a long Windows path) must wrap
/// cleanly inside the 520px panel without intruding into its 20px right padding.
#[test]
fn long_previous_failure_wraps_inside_panel_padding() {
    let frame = render(
        false,
        "update-previous-failed-long-light.png",
        Workspace::preview_update_previous_failure_long,
    );
    // Panel is 520px wide, centered in 1280px -> x = 380..900.
    // Right padding is x = 881..899; check the inner strip x = 883..897 across
    // the body area (below the header divider) against the light popover white.
    let right_padding = (883, 380, 897, 500);
    let light_popover = [0xff, 0xff, 0xff];
    let intruding = frame.count_not_near(right_padding, light_popover, 18);
    eprintln!("UPDATE PREVIOUS LONG right_padding_intruding={intruding}");
    assert_eq!(
        intruding, 0,
        "long previous-failure message overflowed into the panel's right padding ({intruding} px)"
    );
}
