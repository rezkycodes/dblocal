//! WCAG 2.1 contrast checks against the themes that are actually rendered.
//!
//! Earlier work tested hand-typed constants, so the light theme still failed
//! while the test passed. This module parses the same JSON files the app loads
//! (`assets/themes/*.json`) and checks every text-on-background pair the shell
//! can render.

use std::collections::HashMap;

use serde_json::Value;

use crate::theme::THEME_FILES;

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

    /// The opaque channels of a colour whose alpha may be translucent.
    fn from_hsla(color: gpui_kit::Hsla) -> Option<Self> {
        let rgb = color.to_rgb();
        Some(Self {
            r: (rgb.r * 255.0).round() as u8,
            g: (rgb.g * 255.0).round() as u8,
            b: (rgb.b * 255.0).round() as u8,
        })
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
        let pairs = theme_pairs(&name, &colours)
            .into_iter()
            .chain(derived_pairs(&name, &colours))
            .chain(derived_pairs_from_hsla(&name, &colours))
            .chain(tinted_pairs(&name, &colours))
            .chain(extra_text_pairs(&name, &colours));
        for pair in pairs {
            if pair.ratio < MIN_RATIO {
                failing.push(pair.clone());
            }
            all.push(pair);
        }
    }
    (all, failing)
}

/// Alpha the views use for a status badge or banner fill (`theme.rs` tokens).
const BADGE_ALPHA: f64 = 0.14;

/// Composites `color` at `alpha` over an opaque `base`, as GPUI paints a tinted
/// fill over the surface below it.
fn composite(color: Rgb, alpha: f64, base: Rgb) -> Rgb {
    let mix = |fg: u8, bg: u8| (f64::from(fg) * alpha + f64::from(bg) * (1.0 - alpha)).round() as u8;
    Rgb {
        r: mix(color.r, base.r),
        g: mix(color.g, base.g),
        b: mix(color.b, base.b),
    }
}

/// The status colours a badge or banner paints its **text** with, which is the
/// full-strength semantic colour over its own tinted fill (`F-PLH-02` AC2: the
/// status pill draws `status_glyph` and the label in the status colour on a
/// `tint(colour, 0.14)` fill).
fn tinted_pairs(theme_name: &str, colors: &HashMap<String, String>) -> Vec<CheckedPair> {
    // gpui reads `theme.success` from `success.background`, so that token is the
    // one the pill paints text with; the rest follow the same rule.
    const STEMS: [&str; 6] = ["success", "danger", "warning", "info", "primary", "muted"];
    const SURFACES: [&str; 2] = ["background", "sidebar.background"];
    let mut out = Vec::new();
    for stem in STEMS {
        // `muted` text sits on `muted.background`; the semantic stems use
        // `<stem>.background` as both the text and the tint source.
        let fg_key = if stem == "muted" {
            "muted.foreground"
        } else {
            match stem {
                "success" => "success.background",
                "danger" => "danger.background",
                "warning" => "warning.background",
                "info" => "info.background",
                "primary" => "primary.background",
                _ => continue,
            }
        };
        let Some(fg_hex) = colors.get(fg_key) else {
            continue;
        };
        let Some(fg) = Rgb::parse(fg_hex) else {
            continue;
        };
        for surface in SURFACES {
            let Some(base_hex) = colors.get(surface) else {
                continue;
            };
            let Some(base) = Rgb::parse(base_hex) else {
                continue;
            };
            let fill = composite(fg, BADGE_ALPHA, base);
            out.push(CheckedPair {
                theme: theme_name.to_owned(),
                label: format!("{fg_key} on tinted {stem} over {surface}"),
                fg: fg_hex.clone(),
                bg: format!("#{:02x}{:02x}{:02x}", fill.r, fill.g, fill.b),
                ratio: contrast_ratio(fg, fill),
            });
        }
    }
    out
}

/// Text pairs the screens use that are not an `X.foreground`/`X.background`
/// pair: link text on the page, the active tab and the danger/toast text.
fn extra_text_pairs(theme_name: &str, colors: &HashMap<String, String>) -> Vec<CheckedPair> {
    // (foreground key, background key, label)
    const PAIRS: [(&str, &str, &str); 6] = [
        ("link.foreground", "background", "link on page"),
        ("link.foreground", "sidebar.background", "link on dock"),
        ("tab.active.foreground", "tab.active.background", "active tab"),
        ("danger.background", "background", "danger text on page"),
        ("danger.background", "muted.background", "danger text on card"),
        ("warning.background", "muted.background", "warning text on card"),
    ];
    let mut out = Vec::new();
    for (fg_key, bg_key, label) in PAIRS {
        let (Some(fg_hex), Some(bg_hex)) = (colors.get(fg_key), colors.get(bg_key)) else {
            continue;
        };
        let (Some(fg), Some(bg)) = (Rgb::parse(fg_hex), Rgb::parse(bg_hex)) else {
            continue;
        };
        out.push(CheckedPair {
            theme: theme_name.to_owned(),
            label: label.to_owned(),
            fg: fg_hex.clone(),
            bg: bg_hex.clone(),
            ratio: contrast_ratio(fg, bg),
        });
    }
    out
}

/// The WCAG 2.1 non-text minimum for a focus indicator or a control edge.
#[cfg(test)]
pub(crate) const MIN_NON_TEXT_RATIO: f64 = 3.0;

/// The focus-ring colour against the surfaces it is drawn on.
///
/// The ring is a control edge, not text, so WCAG 2.1 asks for 3:1 (SC 1.4.11),
/// not 4.5:1. It still must be checked: a ring that vanished into the surface
/// would make "focus always visible" (`F-PLH-01` AC1) untrue.
#[cfg(test)]
pub(crate) fn focus_ring_pairs(
    theme_name: &str,
    colors: &HashMap<String, String>,
) -> Vec<CheckedPair> {
    let Some(ring_hex) = colors.get("ring") else {
        return Vec::new();
    };
    let Some(ring) = Rgb::parse(ring_hex) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for surface in ["background", "sidebar.background", "popover.background"] {
        let Some(bg_hex) = colors.get(surface) else {
            continue;
        };
        let Some(bg) = Rgb::parse(bg_hex) else {
            continue;
        };
        out.push(CheckedPair {
            theme: theme_name.to_owned(),
            label: format!("ring on {surface}"),
            fg: ring_hex.clone(),
            bg: bg_hex.clone(),
            ratio: contrast_ratio(ring, bg),
        });
    }
    out
}

/// The tokens the views derive their paints from, per theme.
///
/// [`crate::theme::tokens`] only changes alpha on these colours, so a pair that
/// clears 4.5:1 here clears it in every badge, banner and card that uses it.
/// The derived surfaces are the muted and accent fills a card or a selection
/// row paints behind text.
pub(crate) fn derived_pairs(
    theme_name: &str,
    colors: &HashMap<String, String>,
) -> Vec<CheckedPair> {
    const DERIVED_SURFACES: [&str; 2] = ["muted.background", "sidebar.accent.background"];
    // `gpui_kit` reads `theme.success` from `success.background`, so that token
    // is the one the views paint status text with; the same holds for the rest.
    const DERIVED_TEXT: [&str; 7] = [
        "foreground",
        "muted.foreground",
        "success.background",
        "danger.background",
        "warning.background",
        "info.background",
        "primary.background",
    ];
    let mut out = Vec::new();
    for text_key in DERIVED_TEXT {
        let Some(fg) = colors.get(text_key) else {
            continue;
        };
        for surface in DERIVED_SURFACES {
            let Some(bg) = colors.get(surface) else {
                continue;
            };
            let (Some(fg_rgb), Some(bg_rgb)) = (Rgb::parse(fg), Rgb::parse(bg)) else {
                continue;
            };
            out.push(CheckedPair {
                theme: theme_name.to_owned(),
                label: format!("{text_key} on {surface}"),
                fg: fg.clone(),
                bg: bg.clone(),
                ratio: contrast_ratio(fg_rgb, bg_rgb),
            });
        }
    }
    out
}

/// Cross-checks a subset of the derived pairs through the GPUI colour type.
///
/// The views tint theme colours and hand them to GPUI, so the numbers the
/// checker prints must survive the `Hsla` round trip the renderer performs.
pub(crate) fn derived_pairs_from_hsla(
    theme_name: &str,
    colors: &HashMap<String, String>,
) -> Vec<CheckedPair> {
    let mut out = Vec::new();
    for (fg_key, bg_key) in [
        ("foreground", "background"),
        ("foreground", "muted.background"),
    ] {
        let (Some(fg), Some(bg)) = (colors.get(fg_key), colors.get(bg_key)) else {
            continue;
        };
        let (Some(fg_hsla), Some(bg_hsla)) = (
            Rgb::from_hsla(crate::theme::hex_to_hsla(fg)),
            Rgb::from_hsla(crate::theme::hex_to_hsla(bg)),
        ) else {
            continue;
        };
        out.push(CheckedPair {
            theme: theme_name.to_owned(),
            label: format!("{fg_key} on {bg_key} (hsla)"),
            fg: fg.clone(),
            bg: bg.clone(),
            ratio: contrast_ratio(fg_hsla, bg_hsla),
        });
    }
    out
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

        // The light-theme failure measured in the W0-C review: white on the
        // original #4078f2 primary was 3.88:1, which is why the light primary
        // is now a darker blue.
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
        assert!(names.contains(&crate::theme::DARK_THEME));
        assert!(names.contains(&crate::theme::LIGHT_THEME));
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
    fn derived_surfaces_keep_text_legible_too() {
        let mut failing = Vec::new();
        for (name, colours) in parse_themes() {
            for pair in derived_pairs(&name, &colours) {
                if pair.ratio < MIN_RATIO {
                    failing.push(format!(
                        "{} / {}: {} on {} = {:.2}",
                        pair.theme, pair.label, pair.fg, pair.bg, pair.ratio
                    ));
                }
            }
        }
        assert!(
            failing.is_empty(),
            "derived contrast failures:\n{}",
            failing.join("\n")
        );
    }

    #[test]
    fn the_focus_ring_clears_the_non_text_minimum_on_every_surface() {
        // `F-PLH-01` AC1 requires "focus always visible"; a ring that faded into
        // any surface it is drawn on would make that false. WCAG 2.1 SC 1.4.11
        // asks 3:1 for a control edge, which is the bar here.
        let mut failing = Vec::new();
        let mut checked = 0;
        for (name, colours) in parse_themes() {
            for pair in focus_ring_pairs(&name, &colours) {
                checked += 1;
                if pair.ratio < MIN_NON_TEXT_RATIO {
                    failing.push(format!(
                        "{} / {}: {} on {} = {:.2}",
                        pair.theme, pair.label, pair.fg, pair.bg, pair.ratio
                    ));
                }
            }
        }
        assert!(checked >= 6, "each theme must check the ring on its surfaces");
        assert!(
            failing.is_empty(),
            "focus ring contrast failures (need 3:1):\n{}",
            failing.join("\n")
        );
    }

    #[test]
    fn tinted_status_badges_keep_their_text_readable() {
        // The status pill paints its glyph and word in the status colour on a
        // `tint(colour, 0.14)` fill (`F-PLH-02` AC2). Check the composited pair
        // rather than the flat colour, because that is what is on screen.
        let mut failing = Vec::new();
        let mut checked = 0;
        for (name, colours) in parse_themes() {
            for pair in tinted_pairs(&name, &colours) {
                checked += 1;
                if pair.ratio < MIN_RATIO {
                    failing.push(format!(
                        "{} / {}: {} on {} = {:.2}",
                        pair.theme, pair.label, pair.fg, pair.bg, pair.ratio
                    ));
                }
            }
        }
        assert!(checked >= 20, "expected a badge matrix, got {checked}");
        assert!(
            failing.is_empty(),
            "tinted badge contrast failures:\n{}",
            failing.join("\n")
        );
    }

    #[test]
    fn token_table_renders_markdown() {
        let (all, _) = check_all();
        let table = render_table(&all);
        assert!(table.contains("| Theme | Pair |"));
        assert!(table.contains(crate::theme::DARK_THEME));
        assert!(table.contains(crate::theme::LIGHT_THEME));
    }

    #[test]
    fn light_theme_is_not_the_dark_theme() {
        let themes = parse_themes();
        let dark = themes
            .iter()
            .find(|(n, _)| n == crate::theme::DARK_THEME)
            .map(|(_, c)| c)
            .expect("dark");
        let light = themes
            .iter()
            .find(|(n, _)| n == crate::theme::LIGHT_THEME)
            .map(|(_, c)| c)
            .expect("light");
        assert_ne!(dark["background"], light["background"]);
        let dark_l = relative_luminance(Rgb::parse(&dark["background"]).expect("dark bg"));
        let light_l = relative_luminance(Rgb::parse(&light["background"]).expect("light bg"));
        assert!(light_l > dark_l, "light background must be lighter");
    }

    #[test]
    fn derived_surfaces_are_the_tokens_the_views_use() {
        // The views tint only these two surfaces behind text, so the checker
        // must cover them or a badge could regress silently.
        let themes = parse_themes();
        for (name, colours) in &themes {
            let pairs = derived_pairs(name, colours);
            assert!(
                pairs.len() >= 14,
                "{name}: expected every text/surface pair"
            );
            assert!(
                pairs.iter().any(|p| p.label.contains("muted.foreground")
                    && p.label.contains("muted.background")),
                "{name}: muted text on the elevated surface is missing"
            );
        }
    }

    #[test]
    fn hsla_round_trip_feeds_the_same_math() {
        let color = crate::theme::hex_to_hsla("#15161d");
        let rgb = Rgb::from_hsla(color).expect("rgb");
        assert!(rgb.r < 32 && rgb.g < 32 && rgb.b < 40);
        let bg = Rgb::parse("#15161d").expect("bg");
        assert!((contrast_ratio(bg, bg) - 1.0).abs() < 0.001);
    }
}
