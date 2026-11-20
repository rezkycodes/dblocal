//! WCAG contrast checks for the DBLocal Zed-style themes.
//!
//! The PRD (F-PLH-02) requires text contrast >= 4.5:1. Zed's One Dark
//! secondary greys (#5c6370, #4b5263) fail on #282c34; this module documents
//! the adjusted tokens and provides a unit test that fails if any
//! text-on-background pair drops below 4.5:1 (disabled controls exempt).

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    pub const fn hex(hex: u32) -> Self {
        Self {
            r: ((hex >> 16) & 0xff) as u8,
            g: ((hex >> 8) & 0xff) as u8,
            b: (hex & 0xff) as u8,
        }
    }
}

fn linearize(channel: u8) -> f64 {
    let c = f64::from(channel) / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Relative luminance per WCAG 2.1.
pub fn relative_luminance(color: Rgb) -> f64 {
    0.2126 * linearize(color.r) + 0.7152 * linearize(color.g) + 0.0722 * linearize(color.b)
}

/// WCAG contrast ratio between two colours.
pub fn contrast_ratio(fg: Rgb, bg: Rgb) -> f64 {
    let l1 = relative_luminance(fg);
    let l2 = relative_luminance(bg);
    let (hi, lo) = if l1 >= l2 { (l1, l2) } else { (l2, l1) };
    (hi + 0.05) / (lo + 0.05)
}

/// A text-on-background pair that must meet the contrast requirement.
#[derive(Debug, Clone, Copy)]
pub struct ContrastPair {
    pub label: &'static str,
    pub fg: Rgb,
    pub bg: Rgb,
    /// WCAG minimum (4.5 for normal text, 3.0 for large/bold/UI in some cases;
    /// we enforce 4.5 for all non-disabled text per F-PLH-02).
    pub min_ratio: f64,
}

pub fn dark_pairs() -> Vec<ContrastPair> {
    let editor = Rgb::hex(0x282c34);
    let panel = Rgb::hex(0x21252b);
    let tab_active = Rgb::hex(0x282c34);
    let tab_bar = Rgb::hex(0x21252b);
    let popover = Rgb::hex(0x21252b);
    let sidebar = Rgb::hex(0x21252b);
    let status = Rgb::hex(0x21252b);
    let list_active = Rgb::hex(0x2c313a);
    vec![
        ContrastPair {
            label: "dark/body on editor",
            fg: Rgb::hex(0xabb2bf),
            bg: editor,
            min_ratio: 4.5,
        },
        ContrastPair {
            label: "dark/secondary on editor",
            fg: Rgb::hex(0x9da5b4),
            bg: editor,
            min_ratio: 4.5,
        },
        ContrastPair {
            label: "dark/body on panel",
            fg: Rgb::hex(0xabb2bf),
            bg: panel,
            min_ratio: 4.5,
        },
        ContrastPair {
            label: "dark/secondary on panel",
            fg: Rgb::hex(0x9da5b4),
            bg: panel,
            min_ratio: 4.5,
        },
        ContrastPair {
            label: "dark/active tab fg on active tab",
            fg: Rgb::hex(0xabb2bf),
            bg: tab_active,
            min_ratio: 4.5,
        },
        ContrastPair {
            label: "dark/inactive tab fg on tab bar",
            fg: Rgb::hex(0x9da5b4),
            bg: tab_bar,
            min_ratio: 4.5,
        },
        ContrastPair {
            label: "dark/body on status bar",
            fg: Rgb::hex(0xabb2bf),
            bg: status,
            min_ratio: 4.5,
        },
        ContrastPair {
            label: "dark/secondary on status bar",
            fg: Rgb::hex(0x9da5b4),
            bg: status,
            min_ratio: 4.5,
        },
        ContrastPair {
            label: "dark/popover fg",
            fg: Rgb::hex(0xabb2bf),
            bg: popover,
            min_ratio: 4.5,
        },
        ContrastPair {
            label: "dark/sidebar fg",
            fg: Rgb::hex(0xabb2bf),
            bg: sidebar,
            min_ratio: 4.5,
        },
        ContrastPair {
            label: "dark/active row text",
            fg: Rgb::hex(0xabb2bf),
            bg: list_active,
            min_ratio: 4.5,
        },
        ContrastPair {
            label: "dark/running green on editor",
            fg: Rgb::hex(0x98c379),
            bg: editor,
            min_ratio: 4.5,
        },
        ContrastPair {
            label: "dark/warning yellow on editor",
            fg: Rgb::hex(0xe5c07b),
            bg: editor,
            min_ratio: 4.5,
        },
        ContrastPair {
            label: "dark/failed red on editor (adjusted)",
            fg: Rgb::hex(0xe89aa3),
            bg: editor,
            min_ratio: 4.5,
        },
        ContrastPair {
            label: "dark/accent blue on editor",
            fg: Rgb::hex(0x61afef),
            bg: editor,
            min_ratio: 4.5,
        },
        ContrastPair {
            label: "dark/running green on panel",
            fg: Rgb::hex(0x98c379),
            bg: panel,
            min_ratio: 4.5,
        },
        ContrastPair {
            label: "dark/failed red on panel (adjusted)",
            fg: Rgb::hex(0xe89aa3),
            bg: panel,
            min_ratio: 4.5,
        },
        ContrastPair {
            label: "dark/accent blue on panel",
            fg: Rgb::hex(0x61afef),
            bg: panel,
            min_ratio: 4.5,
        },
    ]
}

pub fn light_pairs() -> Vec<ContrastPair> {
    let editor = Rgb::hex(0xfafafa);
    let panel = Rgb::hex(0xf0f0f0);
    let tab_active = Rgb::hex(0xfafafa);
    let tab_bar = Rgb::hex(0xeaeaeb);
    let popover = Rgb::hex(0xfafafa);
    let sidebar = Rgb::hex(0xf0f0f0);
    let status = Rgb::hex(0xf0f0f0);
    let list_active = Rgb::hex(0xe5e5e6);
    vec![
        ContrastPair {
            label: "light/body on editor",
            fg: Rgb::hex(0x383a42),
            bg: editor,
            min_ratio: 4.5,
        },
        ContrastPair {
            label: "light/secondary on editor",
            fg: Rgb::hex(0x696c77),
            bg: editor,
            min_ratio: 4.5,
        },
        ContrastPair {
            label: "light/body on panel",
            fg: Rgb::hex(0x383a42),
            bg: panel,
            min_ratio: 4.5,
        },
        ContrastPair {
            label: "light/secondary on panel",
            fg: Rgb::hex(0x696c77),
            bg: panel,
            min_ratio: 4.5,
        },
        ContrastPair {
            label: "light/active tab fg on active tab",
            fg: Rgb::hex(0x383a42),
            bg: tab_active,
            min_ratio: 4.5,
        },
        ContrastPair {
            label: "light/inactive tab fg on tab bar",
            fg: Rgb::hex(0x5c5f6b),
            bg: tab_bar,
            min_ratio: 4.5,
        },
        ContrastPair {
            label: "light/body on status bar",
            fg: Rgb::hex(0x383a42),
            bg: status,
            min_ratio: 4.5,
        },
        ContrastPair {
            label: "light/secondary on status bar",
            fg: Rgb::hex(0x696c77),
            bg: status,
            min_ratio: 4.5,
        },
        ContrastPair {
            label: "light/popover fg",
            fg: Rgb::hex(0x383a42),
            bg: popover,
            min_ratio: 4.5,
        },
        ContrastPair {
            label: "light/sidebar fg",
            fg: Rgb::hex(0x383a42),
            bg: sidebar,
            min_ratio: 4.5,
        },
        ContrastPair {
            label: "light/active row text",
            fg: Rgb::hex(0x383a42),
            bg: list_active,
            min_ratio: 4.5,
        },
        ContrastPair {
            label: "light/running green on editor",
            fg: Rgb::hex(0x2b7230),
            bg: editor,
            min_ratio: 4.5,
        },
        ContrastPair {
            label: "light/warning yellow on editor",
            fg: Rgb::hex(0x856000),
            bg: editor,
            min_ratio: 4.5,
        },
        ContrastPair {
            label: "light/failed red on editor",
            fg: Rgb::hex(0xb03730),
            bg: editor,
            min_ratio: 4.5,
        },
        ContrastPair {
            label: "light/accent blue on editor",
            fg: Rgb::hex(0x2a56c0),
            bg: editor,
            min_ratio: 4.5,
        },
        ContrastPair {
            label: "light/running green on panel",
            fg: Rgb::hex(0x2b7230),
            bg: panel,
            min_ratio: 4.5,
        },
        ContrastPair {
            label: "light/failed red on panel",
            fg: Rgb::hex(0xb03730),
            bg: panel,
            min_ratio: 4.5,
        },
        ContrastPair {
            label: "light/accent blue on panel",
            fg: Rgb::hex(0x2a56c0),
            bg: panel,
            min_ratio: 4.5,
        },
    ]
}

/// Returns pairs that fail their minimum contrast ratio.
pub fn failing_pairs() -> Vec<(ContrastPair, f64)> {
    let mut failures = Vec::new();
    for pair in dark_pairs().into_iter().chain(light_pairs()) {
        let ratio = contrast_ratio(pair.fg, pair.bg);
        if ratio < pair.min_ratio {
            failures.push((pair, ratio));
        }
    }
    failures
}

/// Format the token table for docs/reports.
pub fn token_table() -> String {
    let mut out = String::new();
    out.push_str("| Theme | Token | Foreground | Background | Ratio | Result |\n");
    out.push_str("|---|---|---|---|---:|---|\n");
    for pair in dark_pairs().into_iter().chain(light_pairs()) {
        let ratio = contrast_ratio(pair.fg, pair.bg);
        let result = if ratio >= pair.min_ratio {
            "pass"
        } else {
            "fail"
        };
        out.push_str(&format!(
            "| {} | {} | #{:02x}{:02x}{:02x} | #{:02x}{:02x}{:02x} | {:.2} | {} |\n",
            if pair.label.starts_with("dark") {
                "dark"
            } else {
                "light"
            },
            pair.label,
            pair.fg.r,
            pair.fg.g,
            pair.fg.b,
            pair.bg.r,
            pair.bg.g,
            pair.bg.b,
            ratio,
            result,
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zed_reference_greys_fail_as_documented() {
        let editor = Rgb::hex(0x282c34);
        assert!(contrast_ratio(Rgb::hex(0x5c6370), editor) < 3.0); // muted/comment in Zed
        assert!(contrast_ratio(Rgb::hex(0x4b5263), editor) < 2.0); // gutter in Zed
        assert!(contrast_ratio(Rgb::hex(0xabb2bf), editor) > 4.5); // body passes
        assert!(contrast_ratio(Rgb::hex(0x9da5b4), editor) > 4.5); // adjusted secondary passes
    }

    #[test]
    fn all_text_pairs_meet_wcag_aa() {
        let failures = failing_pairs();
        if !failures.is_empty() {
            let details: Vec<String> = failures
                .iter()
                .map(|(p, r)| format!("{}: {:.2} < {:.1}", p.label, r, p.min_ratio))
                .collect();
            panic!("contrast failures:\n{}", details.join("\n"));
        }
    }

    #[test]
    fn token_table_is_complete() {
        let table = token_table();
        assert!(table.contains("dark/body on editor"));
        assert!(table.contains("light/body on editor"));
        assert!(table.contains("| pass |") || table.contains("| fail |"));
    }
}

#[cfg(test)]
mod report {
    use super::*;

    /// Prints the token table so the README can quote real numbers.
    #[test]
    fn print_token_table() {
        eprintln!("\n{}", token_table());
    }
}
