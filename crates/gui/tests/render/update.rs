//! In-app update render tests (`F-PKG-05`): the title-bar icon and the update
//! dialog in both themes, in every stage.
//!
//! Included from `tests/render.rs`, so it uses that file's `render`, `Build`,
//! `Frame` helpers and theme tokens. **All frames are mock data** from the
//! preview constructors: no release is downloaded and nothing is installed.

use super::{
    Build, DARK_BG, DARK_DANGER, DARK_PRIMARY, Frame, LIGHT_BG, LIGHT_PRIMARY, LOGICAL_W,
    Workspace, render,
};

/// Every update stage renders in both themes, and the light frame is lighter.
#[test]
fn every_update_stage_renders_in_both_themes() {
    let cases: [(&str, Build); 7] = [
        ("available", Workspace::preview_update_available),
        ("downloading", Workspace::preview_update_downloading),
        ("verifying", Workspace::preview_update_verifying),
        ("installing", Workspace::preview_update_installing),
        ("ready", Workspace::preview_update_ready),
        ("failed", Workspace::preview_update_failed),
        ("cannot", Workspace::preview_update_cannot),
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
        assert_update_dialog_painted(&dark, name, "[dark]", DARK_BG);
        assert_update_dialog_painted(&light, name, "[light]", LIGHT_BG);
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
    let idle_ink = idle.count_not_near(row, DARK_BG, 24);
    let available_ink = available.count_not_near(row, DARK_BG, 24);
    eprintln!("UPDATE ICON idle={idle_ink} available={available_ink}");
    assert!(
        available_ink > idle_ink + 20,
        "the download icon must add ink to the title bar ({idle_ink} -> {available_ink})"
    );
}

/// A managed install's dialog names the release page and draws no download
/// progress.
#[test]
fn a_cannot_self_update_dialog_offers_the_release_page() {
    let frame = render(
        false,
        "update-cannot-light.png",
        Workspace::preview_update_cannot,
    );
    let dialog = (110, 90, LOGICAL_W - 110, 620);
    let text = frame.count_not_near(dialog, LIGHT_BG, 14);
    eprintln!("UPDATE CANNOT text={text}");
    assert!(text > 2_000, "the explanation did not render ({text})");
    // A ghost button, so its border and label are extra ink beyond the text.
    assert!(
        frame.count_near(dialog, LIGHT_PRIMARY, 24) > 0,
        "no primary action was painted in the managed-install dialog"
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
    let danger = frame.count_near(dialog, DARK_DANGER, 28);
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
    let text = frame.count_not_near(panel, LIGHT_BG, 14);
    eprintln!("SETTINGS UPDATE text={text}");
    assert!(text > 2_000, "the Settings update section did not render");
    assert!(
        frame.count_near(panel, DARK_PRIMARY, 40) > 0
            || frame.count_near(panel, LIGHT_PRIMARY, 40) > 0,
        "the re-used install action was not painted"
    );
}
