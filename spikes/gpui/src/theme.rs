use gpui::{App, Hsla, SharedString};
use gpui_component::theme::ThemeRegistry;

/// JSON theme set containing DBLocal One Dark and One Light.
///
/// Based on Atom One Dark / One Light palettes (MIT). Tokens that fall below
/// 4.5:1 WCAG contrast on their intended backgrounds are lightened/darkened;
/// the final values are verified by `contrast::tests`.
pub const THEME_SET_JSON: &str = r##"{
  "$schema": "https://github.com/longbridge/gpui-kit/raw/refs/heads/main/.theme-schema.json",
  "name": "DBLocal Zed",
  "author": "DBLocal",
  "themes": [
    {
      "name": "DBLocal One Dark",
      "mode": "dark",
      "colors": {
        "accent.background": "#2c313a",
        "accent.foreground": "#abb2bf",
        "accordion.background": "#21252b",
        "background": "#282c34",
        "border": "#3e4451",
        "group_box.background": "#21252b",
        "group_box.foreground": "#abb2bf",
        "caret": "#528bff",
        "chart.1": "#61afef",
        "chart.2": "#98c379",
        "chart.3": "#e5c07b",
        "chart.4": "#c678dd",
        "chart.5": "#56b6c2",
        "chart.grid": "#3e4451",
        "danger.background": "#e89aa3",
        "danger.foreground": "#21252b",
        "description_list_label.foreground": "#abb2bf",
        "drag_border": "#528bff",
        "drop_target.background": "#61afef40",
        "foreground": "#abb2bf",
        "info.background": "#61afef",
        "info.foreground": "#21252b",
        "input.border": "#3e4451",
        "link.foreground": "#61afef",
        "link.active.foreground": "#61afef",
        "link.hover.foreground": "#84c5f4",
        "list.background": "#21252b",
        "list.active.background": "#2c313a",
        "list.active.border": "#528bff",
        "list.even.background": "#242930",
        "list.head.background": "#21252b",
        "list.hover.background": "#2c313a",
        "list.highlight.foreground": "#61afef",
        "muted.background": "#21252b",
        "muted.foreground": "#9da5b4",
        "popover.background": "#21252b",
        "popover.foreground": "#abb2bf",
        "primary.background": "#61afef",
        "primary.active.background": "#528bff",
        "primary.foreground": "#21252b",
        "primary.hover.background": "#84c5f4",
        "progress_bar.background": "#61afef",
        "ring": "#528bff",
        "scrollbar.background": "#282c3400",
        "scrollbar.thumb.background": "#4b5263cc",
        "scrollbar.thumb.hover.background": "#5c6370",
        "secondary.background": "#2c313a",
        "secondary.active.background": "#3e4451",
        "secondary.foreground": "#abb2bf",
        "secondary.hover.background": "#353b45",
        "selection.background": "#3e4451",
        "sidebar.background": "#21252b",
        "sidebar.accent.background": "#2c313a",
        "sidebar.accent.foreground": "#abb2bf",
        "sidebar.border": "#3e4451",
        "sidebar.foreground": "#abb2bf",
        "sidebar.primary.background": "#61afef",
        "sidebar.primary.foreground": "#21252b",
        "skeleton.background": "#2c313a",
        "slider.bar.background": "#61afef",
        "slider.thumb.background": "#abb2bf",
        "success.background": "#98c379",
        "success.foreground": "#21252b",
        "switch.background": "#3e4451",
        "tab.background": "#21252b",
        "tab.active.background": "#282c34",
        "tab.active.foreground": "#abb2bf",
        "tab_bar.background": "#21252b",
        "tab_bar.segmented.background": "#21252b",
        "tab.foreground": "#9da5b4",
        "table.background": "#282c34",
        "table.active.background": "#2c313a",
        "table.active.border": "#528bff",
        "table.even.background": "#242930",
        "table.head.background": "#21252b",
        "table.head.foreground": "#9da5b4",
        "table.hover.background": "#2c313a",
        "table.row.border": "#3e445180",
        "title_bar.background": "#21252b",
        "title_bar.border": "#3e4451",
        "status_bar.background": "#21252b",
        "status_bar.border": "#3e4451",
        "warning.background": "#e5c07b",
        "warning.foreground": "#21252b",
        "overlay": "#00000066",
        "window.border": "#3e4451",
        "base.red": "#e89aa3",
        "base.red.light": "#f0b5bc",
        "base.green": "#98c379",
        "base.green.light": "#b5d99a",
        "base.blue": "#61afef",
        "base.blue.light": "#84c5f4",
        "base.yellow": "#e5c07b",
        "base.yellow.light": "#edd4a6",
        "base.magenta": "#c678dd",
        "base.magenta.light": "#dba1eb",
        "base.cyan": "#56b6c2",
        "base.cyan.light": "#7fd0da"
      },
      "highlight": {
        "editor.foreground": "#abb2bf",
        "editor.background": "#282c34",
        "editor.active_line.background": "#2c313a",
        "editor.line_number": "#636d83",
        "editor.active_line_number": "#abb2bf",
        "editor.invisible": "#3e4451",
        "conflict": "#e89aa3",
        "created": "#98c379",
        "deleted": "#e89aa3",
        "hidden": "#636d83",
        "hint": "#61afef",
        "modified": "#e5c07b",
        "predictive": "#636d83",
        "warning": "#e5c07b",
        "syntax": {
          "attribute": { "color": "#d19a66" },
          "boolean": { "color": "#d19a66" },
          "comment": { "color": "#7f848e" },
          "comment.doc": { "color": "#7f848e" },
          "constant": { "color": "#d19a66" },
          "constructor": { "color": "#e5c07b" },
          "embedded": { "color": "#abb2bf" },
          "function": { "color": "#61afef" },
          "keyword": { "color": "#c678dd" },
          "number": { "color": "#d19a66" },
          "string": { "color": "#98c379" },
          "string.escape": { "color": "#56b6c2" },
          "string.regex": { "color": "#56b6c2" },
          "tag": { "color": "#e89aa3" },
          "type": { "color": "#e5c07b" },
          "variable": { "color": "#abb2bf" },
          "variable.special": { "color": "#e89aa3" },
          "property": { "color": "#e89aa3" }
        }
      }
    },
    {
      "name": "DBLocal One Light",
      "mode": "light",
      "colors": {
        "accent.background": "#e5e5e6",
        "accent.foreground": "#383a42",
        "accordion.background": "#f0f0f0",
        "background": "#fafafa",
        "border": "#d0d0d2",
        "group_box.background": "#f0f0f0",
        "group_box.foreground": "#383a42",
        "caret": "#526fff",
        "chart.1": "#4078f2",
        "chart.2": "#2b7230",
        "chart.3": "#856000",
        "chart.4": "#8c1f8a",
        "chart.5": "#01739f",
        "chart.grid": "#d0d0d2",
        "danger.background": "#b03730",
        "danger.foreground": "#fafafa",
        "description_list_label.foreground": "#383a42",
        "drag_border": "#526fff",
        "drop_target.background": "#4078f240",
        "foreground": "#383a42",
        "info.background": "#4078f2",
        "info.foreground": "#fafafa",
        "input.border": "#d0d0d2",
        "link.foreground": "#4078f2",
        "link.active.foreground": "#4078f2",
        "link.hover.foreground": "#2a56c0",
        "list.background": "#f0f0f0",
        "list.active.background": "#e5e5e6",
        "list.active.border": "#526fff",
        "list.even.background": "#f5f5f5",
        "list.head.background": "#eaeaeb",
        "list.hover.background": "#e5e5e6",
        "list.highlight.foreground": "#4078f2",
        "muted.background": "#eaeaeb",
        "muted.foreground": "#696c77",
        "popover.background": "#fafafa",
        "popover.foreground": "#383a42",
        "primary.background": "#4078f2",
        "primary.active.background": "#2f5fd0",
        "primary.foreground": "#fafafa",
        "primary.hover.background": "#5b8cf5",
        "progress_bar.background": "#4078f2",
        "ring": "#526fff",
        "scrollbar.background": "#fafafa00",
        "scrollbar.thumb.background": "#a0a1a7cc",
        "scrollbar.thumb.hover.background": "#696c77",
        "secondary.background": "#eaeaeb",
        "secondary.active.background": "#d0d0d2",
        "secondary.foreground": "#383a42",
        "secondary.hover.background": "#e0e0e2",
        "selection.background": "#d7e3ff",
        "sidebar.background": "#f0f0f0",
        "sidebar.accent.background": "#e5e5e6",
        "sidebar.accent.foreground": "#383a42",
        "sidebar.border": "#d0d0d2",
        "sidebar.foreground": "#383a42",
        "sidebar.primary.background": "#4078f2",
        "sidebar.primary.foreground": "#fafafa",
        "skeleton.background": "#e5e5e6",
        "slider.bar.background": "#4078f2",
        "slider.thumb.background": "#fafafa",
        "success.background": "#2b7230",
        "success.foreground": "#fafafa",
        "switch.background": "#d0d0d2",
        "tab.background": "#eaeaeb",
        "tab.active.background": "#fafafa",
        "tab.active.foreground": "#383a42",
        "tab_bar.background": "#eaeaeb",
        "tab_bar.segmented.background": "#eaeaeb",
        "tab.foreground": "#5c5f6b",
        "table.background": "#fafafa",
        "table.active.background": "#e5e5e6",
        "table.active.border": "#526fff",
        "table.even.background": "#f5f5f5",
        "table.head.background": "#f0f0f0",
        "table.head.foreground": "#696c77",
        "table.hover.background": "#e5e5e6",
        "table.row.border": "#d0d0d280",
        "title_bar.background": "#f0f0f0",
        "title_bar.border": "#d0d0d2",
        "status_bar.background": "#f0f0f0",
        "status_bar.border": "#d0d0d2",
        "warning.background": "#856000",
        "warning.foreground": "#fafafa",
        "overlay": "#00000033",
        "window.border": "#d0d0d2",
        "base.red": "#b03730",
        "base.red.light": "#c85f58",
        "base.green": "#2b7230",
        "base.green.light": "#4a934e",
        "base.blue": "#4078f2",
        "base.blue.light": "#6a98f5",
        "base.yellow": "#856000",
        "base.yellow.light": "#a67a10",
        "base.magenta": "#8c1f8a",
        "base.magenta.light": "#c04ebe",
        "base.cyan": "#01739f",
        "base.cyan.light": "#2aa0d1"
      },
      "highlight": {
        "editor.foreground": "#383a42",
        "editor.background": "#fafafa",
        "editor.active_line.background": "#f0f0f0",
        "editor.line_number": "#8a8a8c",
        "editor.active_line_number": "#383a42",
        "editor.invisible": "#c8c8ca",
        "conflict": "#b03730",
        "created": "#2b7230",
        "deleted": "#b03730",
        "hidden": "#9d9d9f",
        "hint": "#2a56c0",
        "modified": "#856000",
        "predictive": "#9d9d9f",
        "warning": "#856000",
        "syntax": {
          "attribute": { "color": "#856000" },
          "boolean": { "color": "#856000" },
          "comment": { "color": "#696c77" },
          "comment.doc": { "color": "#696c77" },
          "constant": { "color": "#856000" },
          "constructor": { "color": "#9a6a06" },
          "embedded": { "color": "#383a42" },
          "function": { "color": "#2a56c0" },
          "keyword": { "color": "#a626a4" },
          "number": { "color": "#856000" },
          "string": { "color": "#2b7230" },
          "string.escape": { "color": "#01739f" },
          "string.regex": { "color": "#01739f" },
          "tag": { "color": "#b03730" },
          "type": { "color": "#9a6a06" },
          "variable": { "color": "#383a42" },
          "variable.special": { "color": "#b03730" },
          "property": { "color": "#b03730" }
        }
      }
    }
  ]
}"##;

pub const DARK_THEME: &str = "DBLocal One Dark";
pub const LIGHT_THEME: &str = "DBLocal One Light";

/// Register the DBLocal Zed-style themes and select one matching the system appearance.
pub fn init(cx: &mut App) {
    let _ = ThemeRegistry::global_mut(cx).load_themes_from_str(THEME_SET_JSON);
    sync_to_system(cx);
}

/// Install and activate one of the DBLocal themes by name.
///
/// `Theme::apply_config` both registers the file and switches to its mode, so
/// the colors, fonts and radius of that theme land in one step.
pub fn apply_named(cx: &mut App, name: &str) {
    let config = ThemeRegistry::global(cx)
        .themes()
        .get(&SharedString::from(name.to_string()))
        .cloned();
    if let Some(config) = config {
        gpui_component::theme::Theme::update(cx, |theme| theme.apply_config(&config));
    }
}

/// Apply the theme matching the current system appearance.
pub fn sync_to_system(cx: &mut App) {
    let appearance = cx.window_appearance();
    let is_dark = matches!(
        appearance,
        gpui::WindowAppearance::Dark | gpui::WindowAppearance::VibrantDark
    );
    let dark_config = ThemeRegistry::global(cx)
        .themes()
        .get(&SharedString::from(DARK_THEME))
        .cloned();
    let light_config = ThemeRegistry::global(cx)
        .themes()
        .get(&SharedString::from(LIGHT_THEME))
        .cloned();

    let theme = gpui_component::theme::Theme::global_mut(cx);
    if is_dark {
        if let Some(config) = dark_config {
            theme.dark_theme = config;
        }
    } else if let Some(config) = light_config {
        theme.light_theme = config;
    }
    gpui_component::theme::Theme::change(appearance, None, cx);
}

/// Parse a hex color like "#aabbcc" or "aabbcc" into an Hsla for GPUI.
pub fn hex_to_hsla(hex: &str) -> Hsla {
    let h = hex.trim_start_matches('#');
    let (r, g, b, a) = if h.len() == 8 {
        (
            u8::from_str_radix(&h[0..2], 16).unwrap_or(0),
            u8::from_str_radix(&h[2..4], 16).unwrap_or(0),
            u8::from_str_radix(&h[4..6], 16).unwrap_or(0),
            u8::from_str_radix(&h[6..8], 16).unwrap_or(255),
        )
    } else {
        (
            u8::from_str_radix(&h[0..2], 16).unwrap_or(0),
            u8::from_str_radix(&h[2..4], 16).unwrap_or(0),
            u8::from_str_radix(&h[4..6], 16).unwrap_or(0),
            255,
        )
    };
    gpui::rgba(((r as u32) << 24) | ((g as u32) << 16) | ((b as u32) << 8) | a as u32).into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_component::theme::ThemeRegistry;

    #[test]
    fn theme_set_loads_into_registry() {
        let mut registry = ThemeRegistry::default();
        registry
            .load_themes_from_str(THEME_SET_JSON)
            .expect("theme set must load");
        let themes = registry.themes();
        assert!(
            themes.contains_key(&SharedString::from(DARK_THEME)),
            "dark theme missing"
        );
        assert!(
            themes.contains_key(&SharedString::from(LIGHT_THEME)),
            "light theme missing"
        );
        let dark = themes.get(&SharedString::from(DARK_THEME)).unwrap();
        let light = themes.get(&SharedString::from(LIGHT_THEME)).unwrap();
        assert!(dark.mode.is_dark());
        assert!(!light.mode.is_dark());
    }

    #[test]
    fn theme_json_contains_expected_core_tokens() {
        // The JSON is the contract: both themes must declare the core surface and
        // text tokens the shell reads, in both modes.
        for token in [
            "\"background\"",
            "\"foreground\"",
            "\"border\"",
            "\"sidebar.background\"",
            "\"tab.active.background\"",
            "\"title_bar.background\"",
            "\"status_bar.background\"",
            "\"muted.foreground\"",
        ] {
            assert!(
                THEME_SET_JSON.matches(token).count() >= 2,
                "token {token} must appear for both themes"
            );
        }
        assert!(THEME_SET_JSON.contains(DARK_THEME));
        assert!(THEME_SET_JSON.contains(LIGHT_THEME));
    }

    #[test]
    fn hex_to_hsla_handles_six_and_eight_digit_forms() {
        let opaque = hex_to_hsla("#282c34");
        let prefixed = hex_to_hsla("282c34");
        assert_eq!(opaque, prefixed);
        assert_eq!(opaque.a, 1.0);
        let translucent = hex_to_hsla("#282c3480");
        assert!((translucent.a - 0.5).abs() < 0.01);
    }

    #[test]
    fn hex_to_hsla_preserves_lightness_order() {
        let dark = hex_to_hsla("#21252b");
        let light = hex_to_hsla("#fafafa");
        assert!(light.l > dark.l, "light background must be lighter");
    }
}
