//! WCAG 2.1 contrast checks against the themes that are actually rendered.
//!
//! Earlier work tested hand-typed constants, so the light theme still failed
//! while the test passed. This module parses the same JSON files the app loads
//! (`assets/themes/*.json`) and checks every text-on-background pair the shell
//! can render.

use std::collections::HashMap;

use serde_json::Value;

/// Theme minimum ratio (`F-PLH-02`: 4.5:1 for normal text).
pub const MIN_RATIO: f64 = 4.5;

/// An sRGB colour parsed from a theme file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Rgb {
    pub(crate) r: u8,
    pub(crate) g: u8,
    pub(crate) b: u8,
}

impl Rgb {
    fn parse(hex: &str) -> Option<Self> {
        let hex = hex.trim().trim_start_matches('#');
        // Theme files may carry an alpha suffix (`#rrggbbaa`); contrast only
        // applies to the opaque channels.
        if hex.len() == 8 {
            let raw = hex.get(0..8)?;
            let r = u8::from_str_radix(&raw[0..2], 16).ok()?;
            let g = u8::from_str_radix(&raw[2..4], 16).ok()?;
            let b = u8::from_str_radix(&raw[4..6], 16).ok()?;
            return Some(Self { r, g, b });
        }
        if hex.len() != 6 {
            return None;
        }
        let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
        let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
        let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
        Some(Self { r, g, b })
    }
}

fn linearize(channel: u8) -> f64 {
    let c = f64::from(channel) / 255.0;
    if c <= 0.040_45 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Relative luminance per WCAG 2.1.
pub(crate) fn relative_luminance(colour: Rgb) -> f64 {
    0.2126 * linearize(colour.r) + 0.7152 * linearize(colour.g) + 0.0722 * linearize(colour.b)
}

/// Contrast ratio between two opaque colours.
pub(crate) fn contrast_ratio(fg: Rgb, bg: Rgb) -> f64 {
    let a = relative_luminance(fg);
    let b = relative_luminance(bg);
    let (hi, lo) = if a >= b { (a, b) } else { (b, a) };
    (hi + 0.05) / (lo + 0.05)
}

/// A single pair the shell renders, plus where it came from.
#[derive(Debug, Clone)]
pub(crate) struct CheckedPair {
    pub(crate) theme: String,
    pub(crate) label: String,
    pub(crate) fg: String,
    pub(crate) bg: String,
    pub(crate) ratio: f64,
}

/// Every colour token pair that must meet [`MIN_RATIO`] for one theme.
///
/// The list mirrors the widget set in `docs/design/UI-DIRECTION.md`: body and
/// muted text on both surfaces, the status colours, the tab/list rows, and
/// every `X.foreground` that is paired with an `X.background`.
pub(crate) fn theme_pairs(theme_name: &str, colors: &HashMap<String, String>) -> Vec<CheckedPair> {
    let mut pairs: Vec<(String, String, String)> = Vec::new();

    let background = colors.get("background").cloned();
    let sidebar = colors.get("sidebar.background").cloned();
    let panel_surfaces = [background.clone(), sidebar.clone()];

    // Body and muted text on the surfaces they actually sit on.
    for (label, fg_key) in [
        ("foreground", "foreground"),
        ("muted.foreground", "muted.foreground"),
    ] {
        if let Some(fg) = colors.get(fg_key) {
            for (ix, bg) in panel_surfaces.iter().enumerate() {
                if let Some(bg) = bg {
                    let surface = if ix == 0 {
                        "background"
                    } else {
                        "sidebar.background"
                    };
                    pairs.push((format!("{label} on {surface}"), fg.clone(), bg.clone()));
                }
            }
        }
    }

    // Status / semantic colours against both base surfaces (F-PLH-02 AC2).
    for stem in [
        "success",
        "danger",
        "warning",
        "info",
        "primary",
        "accent",
        "secondary",
        "link",
    ] {
        if let Some(fg) = colors.get(&format!("{stem}.foreground"))
            && let Some(bg) = colors.get(&format!("{stem}.background"))
        {
            pairs.push((
                format!("{stem}.foreground on {stem}.background"),
                fg.clone(),
                bg.clone(),
            ));
        }
        if let Some(fg) = colors.get(stem) {
            for (ix, bg) in panel_surfaces.iter().enumerate() {
                if let Some(bg) = bg {
                    let surface = if ix == 0 {
                        "background"
                    } else {
                        "sidebar.background"
                    };
                    pairs.push((format!("{stem} on {surface}"), fg.clone(), bg.clone()));
                }
            }
        }
    }

    // Every explicit `X.foreground` / `X.background` pair the theme declares.
    for key in colors.keys() {
        if let Some(stem) = key.strip_suffix(".foreground")
            && let Some(bg) = colors.get(&format!("{stem}.background"))
        {
            pairs.push((
                format!("{stem}.foreground on {stem}.background"),
                colors[key].clone(),
                bg.clone(),
            ));
        }
    }

    // Base palette colours must stay legible on both surfaces, including the
    // `.light` variants the highlighter uses.
    for key in colors.keys() {
        if key.starts_with("base.") {
            for (ix, bg) in panel_surfaces.iter().enumerate() {
                if let Some(bg) = bg {
                    let surface = if ix == 0 {
                        "background"
                    } else {
                        "sidebar.background"
                    };
                    pairs.push((
                        format!("{key} on {surface}"),
                        colors[key].clone(),
                        bg.clone(),
                    ));
                }
            }
        }
    }

    // Tabs and list rows.
    for (fg_key, bg_key, label) in [
        (
            "tab.active.foreground",
            "tab.active.background",
            "tab.active",
        ),
        ("tab.foreground", "tab_bar.background", "tab"),
        (
            "list.active.foreground",
            "list.active.background",
            "list.active",
        ),
        (
            "table.active.foreground",
            "table.active.background",
            "table.active",
        ),
        ("list.head.foreground", "list.head.background", "list.head"),
        (
            "table.head.foreground",
            "table.head.background",
            "table.head",
        ),
    ] {
        if let (Some(fg), Some(bg)) = (colors.get(fg_key), colors.get(bg_key)) {
            pairs.push((format!("{label} text"), fg.clone(), bg.clone()));
        }
    }

    pairs
        .into_iter()
        .filter_map(|(label, fg, bg)| {
            let fg_rgb = Rgb::parse(&fg);
            let bg_rgb = Rgb::parse(&bg);
            match (fg_rgb, bg_rgb) {
                (Some(fg_rgb), Some(bg_rgb)) => Some(CheckedPair {
                    theme: theme_name.to_owned(),
                    label,
                    fg,
                    bg,
                    ratio: contrast_ratio(fg_rgb, bg_rgb),
                }),
                _ => None,
            }
        })
        .collect()
}

/// Themes embedded in the binary, in load order.
pub(crate) const THEME_FILES: [(&str, &str); 2] = [
    (
        "crates/gui/assets/themes/one-dark.json",
        include_str!("../assets/themes/one-dark.json"),
    ),
    (
        "crates/gui/assets/themes/one-light.json",
        include_str!("../assets/themes/one-light.json"),
    ),
];

/// Parses every theme file into `(theme name, colours)` pairs.
pub(crate) fn parse_themes() -> Vec<(String, HashMap<String, String>)> {
    let mut out = Vec::new();
    for (path, text) in THEME_FILES {
        let value: Value = match serde_json::from_str(text) {
            Ok(value) => value,
            Err(err) => {
                tracing::error!("theme file {path} is not valid JSON: {err}");
                continue;
            }
        };
        let Some(themes) = value.get("themes").and_then(Value::as_array) else {
            continue;
        };
        for theme in themes {
            let Some(name) = theme.get("name").and_then(Value::as_str) else {
                continue;
            };
            let Some(colours) = theme.get("colors").and_then(Value::as_object) else {
                continue;
            };
            let map: HashMap<String, String> = colours
                .iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_owned())))
                .collect();
            out.push((name.to_owned(), map));
        }
    }
    out
}

/// Runs every check, returning the full table and the failing rows.
pub(crate) fn check_all() -> (Vec<CheckedPair>, Vec<CheckedPair>) {
    let mut all = Vec::new();
    let mut failing = Vec::new();
    for (name, colours) in parse_themes() {
        for pair in theme_pairs(&name, &colours) {
            if pair.ratio < MIN_RATIO {
                failing.push(pair.clone());
            }
            all.push(pair);
        }
    }
    (all, failing)
}

/// Formats the measured table for docs and the final report.
pub(crate) fn render_table(pairs: &[CheckedPair]) -> String {
    let mut out = String::from("| Theme | Pair | Foreground | Background | Ratio |\n");
    out.push_str("|---|---|---|---|---:|\n");
    for pair in pairs {
        out.push_str(&format!(
            "| {} | {} | {} | {} | {:.2} |\n",
            pair.theme, pair.label, pair.fg, pair.bg, pair.ratio
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wcag_reference_values_are_reproduced() {
        // White on black is the maximum ratio, 21:1.
        let white = Rgb::parse("#ffffff").expect("white");
        let black = Rgb::parse("#000000").expect("black");
        assert!((contrast_ratio(white, black) - 21.0).abs() < 0.01);

        // The light-theme failures measured in the W0-C review.
        let primary_bg = Rgb::parse("#4078f2").expect("blue");
        let on_light = Rgb::parse("#fafafa").expect("near white");
        assert!((contrast_ratio(on_light, primary_bg) - 3.88).abs() < 0.02);
    }

    #[test]
    fn parses_alpha_suffixes_and_rejects_garbage() {
        assert_eq!(
            Rgb::parse("#282c3400"),
            Some(Rgb {
                r: 0x28,
                g: 0x2c,
                b: 0x34
            })
        );
        assert!(Rgb::parse("#abcd").is_none());
        assert!(Rgb::parse("not a colour").is_none());
    }

    #[test]
    fn both_theme_files_load_with_their_tokens() {
        let themes = parse_themes();
        assert_eq!(themes.len(), 2, "dark and light must both load");
        let names: Vec<&str> = themes.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&"DBLocal One Dark"));
        assert!(names.contains(&"DBLocal One Light"));
        for (_, colours) in &themes {
            assert!(colours.contains_key("background"));
            assert!(colours.contains_key("foreground"));
        }
    }

    #[test]
    fn every_rendered_text_pair_meets_wcag_aa() {
        let (all, failing) = check_all();
        assert!(
            all.len() >= 64,
            "expected a broad matrix, got {}",
            all.len()
        );
        if !failing.is_empty() {
            let details: Vec<String> = failing
                .iter()
                .map(|p| {
                    format!(
                        "{} / {}: {} on {} = {:.2}",
                        p.theme, p.label, p.fg, p.bg, p.ratio
                    )
                })
                .collect();
            panic!("contrast failures:\n{}", details.join("\n"));
        }
    }

    #[test]
    fn token_table_renders_markdown() {
        let (all, _) = check_all();
        let table = render_table(&all);
        assert!(table.contains("| Theme | Pair |"));
        assert!(table.contains("DBLocal One Dark"));
        assert!(table.contains("DBLocal One Light"));
    }

    #[test]
    fn light_theme_is_not_the_dark_theme() {
        let themes = parse_themes();
        let dark = themes
            .iter()
            .find(|(n, _)| n == "DBLocal One Dark")
            .map(|(_, c)| c)
            .expect("dark");
        let light = themes
            .iter()
            .find(|(n, _)| n == "DBLocal One Light")
            .map(|(_, c)| c)
            .expect("light");
        assert_ne!(dark["background"], light["background"]);
        let dark_l = relative_luminance(Rgb::parse(&dark["background"]).expect("dark bg"));
        let light_l = relative_luminance(Rgb::parse(&light["background"]).expect("light bg"));
        assert!(light_l > dark_l, "light background must be lighter");
    }
}
