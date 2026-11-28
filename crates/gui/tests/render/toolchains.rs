//! Headless wgpu render tests for the Toolchains GUI (v0.3.0).
//!
//! Captures every required toolchain screen in BOTH dark and light themes at
//! 1280px and 900px widths and writes named PNGs into
//! `~/.cache/dblocal-v030/renders/` for visual inspection. Also verifies that
//! clicking a sidebar toolchain row with the mouse does NOT draw a focus ring
//! (focus rings are keyboard-only).

use std::path::PathBuf;

use dblocal_gui::app::Workspace;

use super::{Build, DOCK, LOGICAL_H, LOGICAL_W, dark_primary, render_clicked, render_sized};

fn v030_render_dir() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    home.join(".cache/dblocal-v030/renders")
}

fn save_v030(frame: &super::Frame, filename: &str) {
    let dir = v030_render_dir();
    std::fs::create_dir_all(&dir).expect("create v030 render dir");
    let path = dir.join(filename);
    frame.image.save(&path).expect("save v030 render png");
}

fn render_both_themes_and_widths(slug: &str, build: Build) {
    for (theme_name, dark) in [("dark", true), ("light", false)] {
        let f1280 = render_sized(
            dark,
            &format!("tc-{slug}-{theme_name}-1280.png"),
            build,
            LOGICAL_W,
            LOGICAL_H,
        );
        assert!(
            f1280.colours > 30,
            "{slug} {theme_name} 1280 render too flat: {}",
            f1280.colours
        );
        save_v030(&f1280, &format!("{slug}-{theme_name}-1280.png"));

        let f900 = render_sized(
            dark,
            &format!("tc-{slug}-{theme_name}-900.png"),
            build,
            900,
            LOGICAL_H,
        );
        assert!(
            f900.colours > 30,
            "{slug} {theme_name} 900 render too flat: {}",
            f900.colours
        );
        save_v030(&f900, &format!("{slug}-{theme_name}-900.png"));
    }
}

#[test]
fn renders_toolchain_screens_in_both_themes_and_widths() {
    let cases: &[(&str, Build)] = &[
        ("php", Workspace::preview_toolchain_php),
        ("node", Workspace::preview_toolchain_node),
        (
            "composer-needs-php",
            Workspace::preview_toolchain_composer_needs_php,
        ),
        (
            "path-not-applied",
            Workspace::preview_toolchain_path_not_applied,
        ),
        ("path-applied", Workspace::preview_toolchain_path_applied),
        ("path-shadowed", Workspace::preview_toolchain_path_shadowed),
        ("path-consent", Workspace::preview_toolchain_path_consent),
        ("remove-confirm", Workspace::preview_toolchain_remove_dialog),
        ("empty-catalog", Workspace::preview_toolchain_empty_catalog),
    ];

    for (slug, build) in cases {
        render_both_themes_and_widths(slug, *build);
    }

    // Save a dedicated sidebar crop showing INSTANCES + TOOLCHAINS + RUNTIMES
    // in both dark and light themes at 1280px.
    for (theme_name, dark) in [("dark", true), ("light", false)] {
        let frame = render_sized(
            dark,
            &format!("tc-sidebar-{theme_name}.png"),
            Workspace::preview_toolchain_php,
            LOGICAL_W,
            LOGICAL_H,
        );
        let crop_w = (DOCK * frame.scale).min(frame.image.width());
        let crop_h = frame.image.height();
        let cropped = image::imageops::crop_imm(&frame.image, 0, 0, crop_w, crop_h).to_image();
        let dir = v030_render_dir();
        std::fs::create_dir_all(&dir).expect("create v030 render dir");
        cropped
            .save(dir.join(format!("sidebar-{theme_name}.png")))
            .expect("save sidebar crop");
    }
}

#[test]
fn renders_php_settings_card_in_both_themes_and_widths() {
    render_both_themes_and_widths(
        "php-settings-error",
        Workspace::preview_toolchain_php_settings_error,
    );
}

#[test]
fn clicking_toolchain_sidebar_row_does_not_draw_focus_ring() {
    // Click on the sidebar area around the Node.js toolchain row; mouse modality
    // must NOT draw a keyboard focus ring around the row.
    let base = render_sized(
        true,
        "tc-click-base.png",
        Workspace::preview_toolchain_php,
        LOGICAL_W,
        LOGICAL_H,
    );
    let clicked = render_clicked(
        true,
        "tc-click-mouse.png",
        Workspace::preview_toolchain_php,
        (80, 460),
    );
    // Primary focus-ring colour pixels in the sidebar should not spike after a
    // mouse click (only the 3px active bar uses primary).
    let base_ring = base.count_near((0, 300, DOCK, 600), dark_primary(), 12);
    let click_ring = clicked.count_near((0, 300, DOCK, 600), dark_primary(), 12);
    assert!(
        click_ring <= base_ring + 40,
        "mouse click drew unexpected focus ring pixels in sidebar: base={base_ring} click={click_ring}"
    );
}
