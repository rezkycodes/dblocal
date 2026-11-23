//! DBLocal themes and the tokens custom views draw with.
//!
//! Two themes ship, `DBLocal Dark` (base `#15161d`, the prototype palette) and
//! `DBLocal Light` in the same visual language. The app follows the system
//! appearance (`F-PLH-02` AC1). Both files are the same ones
//! [`crate::contrast`] parses, so a colour that fails 4.5:1 fails the build.
//!
//! Views must not contain colour literals: every paint comes either from a
//! `gpui_kit::component::Theme` field or from [`tokens`], which derives its
//! values from the active theme and therefore switches with it.

use gpui_kit::component::theme::{Theme, ThemeRegistry};
use gpui_kit::{App, Hsla, SharedString, WindowAppearance};

/// Dark theme name as declared in the JSON.
pub const DARK_THEME: &str = "DBLocal Dark";
/// Light theme name as declared in the JSON.
pub const LIGHT_THEME: &str = "DBLocal Light";

/// Theme JSON files, embedded so the binary needs no data directory.
pub const THEME_FILES: [(&str, &str); 2] = [
    (
        "crates/gui/assets/themes/dblocal-dark.json",
        include_str!("../assets/themes/dblocal-dark.json"),
    ),
    (
        "crates/gui/assets/themes/dblocal-light.json",
        include_str!("../assets/themes/dblocal-light.json"),
    ),
];

/// Parses every embedded theme file into a single document the registry
/// accepts, so both files reach the registry through one call.
pub fn merged_theme_set() -> String {
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
///
/// `F-PLH-02` AC1: the app follows the system appearance. `DBLOCAL_THEME` is an
/// explicit override (`light`, `dark`, or `system`); it exists so the override
/// can be forced in a test or a screenshot run without touching the user's real
/// desktop theme, and it is documented in `docs/architecture/gui.md`.
pub fn init(cx: &mut App) {
    let set = merged_theme_set();
    if let Err(err) = ThemeRegistry::global_mut(cx).load_themes_from_str(&set) {
        tracing::error!("could not register the DBLocal themes: {err}");
    }
    apply_from_environment_or_system(cx);
    apply_explicit_fonts(cx);
    log_resolved_fonts(cx);
}

/// The theme override named by `DBLOCAL_THEME`, if any.
///
/// `light` and `dark` force a theme; `system` (or an unset/garbage value) means
/// "follow the OS". The value is read once per call so a test can set it in the
/// process environment before `init`.
pub fn theme_override() -> Option<&'static str> {
    match std::env::var("DBLOCAL_THEME") {
        Ok(value) if value.eq_ignore_ascii_case("dark") => Some(DARK_THEME),
        Ok(value) if value.eq_ignore_ascii_case("light") => Some(LIGHT_THEME),
        _ => None,
    }
}

/// Applies the override if one is set, else the system appearance.
pub fn apply_from_environment_or_system(cx: &mut App) {
    match theme_override() {
        Some(name) => apply_named(cx, name),
        None => sync_to_system(cx),
    }
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
    let wanted = match cx.window_appearance() {
        WindowAppearance::Dark | WindowAppearance::VibrantDark => DARK_THEME,
        _ => LIGHT_THEME,
    };
    apply_named(cx, wanted);
}

/// Logs the UI and monospace families GPUI actually resolved.
pub fn log_resolved_fonts(cx: &App) {
    let theme = Theme::global(cx);
    let ui = theme.font_family.clone();
    let mono = theme.mono_font_family.clone();
    tracing::info!("UI font family: {ui:?}");
    tracing::info!("Monospace font family: {mono:?}");
    let resolved = cx
        .text_system()
        .resolve_font(&gpui_kit::font(ui.as_ref()))
        .0;
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
    gpui_kit::rgba(((r as u32) << 24) | ((g as u32) << 16) | ((b as u32) << 8) | a as u32).into()
}

/// Derived paints for the views, all taken from the active theme.
///
/// Each function reads a `gpui_kit` theme token and only adjusts alpha, so the
/// same call produces the dark and the light value and no view needs a literal.
pub mod tokens {
    use gpui_kit::component::{ActiveTheme, Theme};
    use gpui_kit::{App, Hsla};

    /// Alpha applied to a status tint behind a badge or banner.
    const TINT: f32 = 0.14;
    /// Alpha applied to a banner border.
    const BORDER_TINT: f32 = 0.45;
    /// Alpha of the modal scrim.
    const SCRIM: f32 = 0.45;

    /// Sets only the alpha of a theme colour.
    pub fn tint(color: Hsla, alpha: f32) -> Hsla {
        let mut out = color;
        out.a = alpha;
        out
    }

    /// The modal backdrop.
    pub fn scrim(cx: &App) -> Hsla {
        tint(cx.theme().background, SCRIM)
    }

    /// The modal backdrop from a theme the caller already holds.
    pub fn scrim_from(theme: &gpui_kit::component::Theme) -> Hsla {
        tint(theme.background, SCRIM)
    }

    /// Surface behind a raised card inside a page.
    pub fn card(theme: &Theme) -> Hsla {
        theme.background
    }

    /// Surface of a dock or a chrome strip.
    pub fn panel(theme: &Theme) -> Hsla {
        theme.sidebar
    }

    /// Elevated surface: popovers, banners inside cards, hover fills.
    pub fn elevated(theme: &Theme) -> Hsla {
        theme.muted
    }

    /// A badge fill for a status colour.
    pub fn badge_fill(color: Hsla) -> Hsla {
        tint(color, TINT)
    }

    /// A banner fill for a semantic colour.
    pub fn banner_fill(color: Hsla) -> Hsla {
        tint(color, TINT)
    }

    /// A banner border for a semantic colour.
    pub fn banner_border(color: Hsla) -> Hsla {
        tint(color, BORDER_TINT)
    }

    /// Selection fill inside a dock or a list.
    pub fn selection(theme: &Theme) -> Hsla {
        theme.sidebar_accent
    }

    /// Hairline border inside a card.
    pub fn hairline(theme: &Theme) -> Hsla {
        theme.table_row_border
    }
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
    fn the_dark_theme_is_the_first_and_default() {
        let merged = merged_theme_set();
        let value: serde_json::Value = serde_json::from_str(&merged).expect("valid json");
        let themes = value["themes"].as_array().expect("themes");
        assert_eq!(themes[0]["name"], DARK_THEME);
        assert_eq!(themes[0]["is_default"], true);
        assert_eq!(themes[1]["name"], LIGHT_THEME);
        assert_eq!(themes[1]["mode"], "light");
    }

    #[test]
    fn dark_theme_uses_the_prototype_base_colour() {
        let merged = merged_theme_set();
        let value: serde_json::Value = serde_json::from_str(&merged).expect("valid json");
        let themes = value["themes"].as_array().expect("themes");
        assert_eq!(themes[0]["colors"]["background"], "#15161d");
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

    #[test]
    fn tints_change_only_alpha() {
        let base = hex_to_hsla("#61afef");
        let soft = tokens::tint(base, 0.14);
        assert!((soft.a - 0.14).abs() < f32::EPSILON);
        assert_eq!((soft.h, soft.s, soft.l), (base.h, base.s, base.l));
    }
}
