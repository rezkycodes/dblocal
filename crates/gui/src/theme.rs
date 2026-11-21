//! Loads the DBLocal One Dark / One Light themes and follows the system
//! appearance. Colors come from the JSON files under `assets/themes/`, the same
//! ones `contrast` verifies.

use gpui::{App, Hsla, SharedString};
use gpui_component::theme::{Theme, ThemeRegistry};

use crate::contrast::THEME_FILES;

/// Dark theme name as declared in the JSON.
pub const DARK_THEME: &str = "DBLocal One Dark";
/// Light theme name as declared in the JSON.
pub const LIGHT_THEME: &str = "DBLocal One Light";

/// Parses every embedded theme file into a single JSON document the registry
/// accepts, so both files reach the registry through one call.
fn merged_theme_set() -> String {
    let mut themes: Vec<serde_json::Value> = Vec::new();
    for (path, text) in THEME_FILES {
        match serde_json::from_str::<serde_json::Value>(text) {
            Ok(value) => {
                if let Some(list) = value.get("themes").and_then(|v| v.as_array()) {
                    themes.extend(list.iter().cloned());
                }
            }
            Err(err) => tracing::error!("theme file {path} is not valid JSON: {err}"),
        }
    }
    serde_json::json!({
        "name": "DBLocal",
        "author": "DBLocal",
        "themes": themes,
    })
    .to_string()
}

/// Registers both themes and selects the one matching the system appearance.
pub fn init(cx: &mut App) {
    let set = merged_theme_set();
    if let Err(err) = ThemeRegistry::global_mut(cx).load_themes_from_str(&set) {
        tracing::error!("could not register the DBLocal themes: {err}");
    }
    sync_to_system(cx);
    apply_explicit_fonts(cx);
    log_resolved_fonts(cx);
}

/// The sane UI font stack, in preference order.
///
/// `.SystemUIFont` is kept as the final fallback (W0-C recommended shipping the
/// system font). On Linux GPUI maps `.SystemUIFont` through a stack that
/// includes Windows and macOS families; if a host happens to have `Segoe UI`
/// installed under `~/.local/share/fonts` — as this review machine does —
/// `.SystemUIFont` resolves to it, which is not the intended Linux look.
/// Naming an installed desktop family explicitly avoids that silent
/// substitution while still following a system font.
pub const UI_FONT_STACK: [&str; 8] = [
    "Adwaita Sans",
    "Cantarell",
    "Noto Sans",
    "Ubuntu",
    "DejaVu Sans",
    "Liberation Sans",
    "FreeSans",
    ".SystemUIFont",
];

/// The monospace stack used for logs, URIs and ports.
pub const MONO_FONT_STACK: [&str; 7] = [
    "Adwaita Mono",
    "Noto Sans Mono",
    "JetBrains Mono",
    "Source Code Pro",
    "DejaVu Sans Mono",
    "Liberation Mono",
    ".SystemUIFont",
];

/// Returns the first family of `stack` that is installed, if any.
pub fn first_installed(stack: &[&str], installed: &[String]) -> Option<SharedString> {
    stack
        .iter()
        .find(|candidate| installed.iter().any(|name| name == *candidate))
        .map(|family| SharedString::from((*family).to_owned()))
}

/// Sets explicit UI and mono families from the installed fonts on Linux.
///
/// Windows and macOS keep their native system font.
fn apply_explicit_fonts(cx: &mut App) {
    if !cfg!(target_os = "linux") {
        return;
    }
    let installed = cx.text_system().all_font_names();
    if installed.is_empty() {
        return;
    }
    let ui = first_installed(&UI_FONT_STACK, &installed);
    let mono = first_installed(&MONO_FONT_STACK, &installed);
    Theme::update(cx, |theme| {
        if let Some(ui) = ui {
            theme.font_family = ui;
        }
        if let Some(mono) = mono {
            theme.mono_font_family = mono;
        }
    });
}

/// Installs one registered theme by name (used by tests and the theme toggle).
pub fn apply_named(cx: &mut App, name: &str) {
    let config = ThemeRegistry::global(cx)
        .themes()
        .get(&SharedString::from(name.to_owned()))
        .cloned();
    if let Some(config) = config {
        Theme::update(cx, |theme| theme.apply_config(&config));
    } else {
        tracing::warn!("theme {name} is not registered");
    }
}

/// Applies the theme that matches the current system appearance.
pub fn sync_to_system(cx: &mut App) {
    let appearance = cx.window_appearance();
    let is_dark = matches!(
        appearance,
        gpui::WindowAppearance::Dark | gpui::WindowAppearance::VibrantDark
    );
    let wanted = if is_dark { DARK_THEME } else { LIGHT_THEME };
    apply_named(cx, wanted);
}

/// Logs the UI and monospace families GPUI actually resolved.
///
/// W0-C reported the system font as "Segoe UI" on a Fedora host, which is a
/// Windows family; this records what the text system truly selects so the
/// claim can be checked instead of assumed.
pub fn log_resolved_fonts(cx: &App) {
    let theme = Theme::global(cx);
    let ui = theme.font_family.clone();
    let mono = theme.mono_font_family.clone();
    tracing::info!("UI font family: {ui:?}");
    tracing::info!("Monospace font family: {mono:?}");
    let resolved = cx.text_system().resolve_font(&gpui::font(ui.as_ref())).0;
    tracing::info!("resolved UI font id: {resolved}");
}

/// Parses a `#rrggbb` or `#rrggbbaa` token into a GPUI colour.
pub fn hex_to_hsla(hex: &str) -> Hsla {
    let h = hex.trim().trim_start_matches('#');
    let parse = |s: &str| u8::from_str_radix(s, 16).unwrap_or(0);
    let (r, g, b, a) = if h.len() == 8 {
        (
            parse(&h[0..2]),
            parse(&h[2..4]),
            parse(&h[4..6]),
            parse(&h[6..8]),
        )
    } else {
        (parse(&h[0..2]), parse(&h[2..4]), parse(&h[4..6]), 255)
    };
    gpui::rgba(((r as u32) << 24) | ((g as u32) << 16) | ((b as u32) << 8) | a as u32).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merged_theme_set_contains_both_themes() {
        let merged = merged_theme_set();
        assert!(merged.contains(DARK_THEME));
        assert!(merged.contains(LIGHT_THEME));
        let value: serde_json::Value = serde_json::from_str(&merged).expect("valid json");
        assert_eq!(value["themes"].as_array().expect("themes").len(), 2);
    }

    #[test]
    fn font_stacks_pick_the_first_installed_family() {
        let installed: Vec<String> =
            vec!["Segoe UI".into(), "Noto Sans".into(), "DejaVu Sans".into()];
        assert_eq!(
            first_installed(&UI_FONT_STACK, &installed),
            Some(SharedString::from("Noto Sans"))
        );
        let mono: Vec<String> = vec!["Noto Sans Mono".into()];
        assert_eq!(
            first_installed(&MONO_FONT_STACK, &mono),
            Some(SharedString::from("Noto Sans Mono"))
        );
    }

    #[test]
    fn font_stacks_fall_back_to_the_system_font() {
        let installed: Vec<String> = vec![".SystemUIFont".into()];
        assert_eq!(
            first_installed(&UI_FONT_STACK, &installed),
            Some(SharedString::from(".SystemUIFont"))
        );
        assert!(first_installed(&UI_FONT_STACK, &[]).is_none());
    }

    #[test]
    fn windows_font_is_never_preferred_over_a_linux_family() {
        // Even when Segoe UI is installed (this review host has it under
        // ~/.local/share/fonts), a desktop family earlier in the stack wins.
        let installed: Vec<String> = vec!["Segoe UI".into(), "Cantarell".into()];
        let picked = first_installed(&UI_FONT_STACK, &installed).expect("family");
        assert_eq!(picked, SharedString::from("Cantarell"));
    }

    #[test]
    fn hex_parsing_handles_six_and_eight_digit_forms() {
        let opaque = hex_to_hsla("#282c34");
        let prefixed = hex_to_hsla("282c34");
        assert_eq!(opaque, prefixed);
        assert!((opaque.a - 1.0).abs() < f32::EPSILON);
        let translucent = hex_to_hsla("#282c3480");
        assert!((translucent.a - 0.5).abs() < 0.01);
    }
}
