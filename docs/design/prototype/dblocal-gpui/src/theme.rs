//! The DBLocal theme: base palette #15161d (derived from One Dark, all text meets 4.5:1 contrast).
//!
//! gpui-kit component colours are set through `themes/dblocal.json`; the constants below
//! are used for our own elements (status dots, engine badges, log panels).

use gpui_kit::component::{Theme, ThemeRegistry};
use gpui_kit::{App, Hsla, Rgba, SharedString, rgb, rgba};

const THEME_JSON: &str = include_str!("../themes/dblocal.json");
const THEME_NAME: &str = "DBLocal Dark";

/// Load the theme from JSON and make it active.
pub fn init(cx: &mut App) {
    if let Err(err) = ThemeRegistry::global_mut(cx).load_themes_from_str(THEME_JSON) {
        eprintln!("[dblocal] could not load the theme: {err:#}");
        return;
    }
    let config = ThemeRegistry::global(cx)
        .themes()
        .get(&SharedString::from(THEME_NAME))
        .cloned();
    if let Some(config) = config {
        Theme::update(cx, |theme| theme.apply_config(&config));
    }
}

/// Colour tokens for custom elements.
pub mod tokens {
    use super::*;

    pub fn bg_editor() -> Rgba {
        rgb(0x15161d)
    }
    pub fn bg_panel() -> Rgba {
        rgb(0x1c1d26)
    }
    pub fn bg_elevated() -> Rgba {
        rgb(0x23242f)
    }
    pub fn bg_hover() -> Rgba {
        rgb(0x2c2e3b)
    }
    pub fn bg_selected() -> Rgba {
        rgb(0x2a2c3a)
    }
    pub fn border() -> Rgba {
        rgb(0x30323f)
    }
    pub fn border_strong() -> Rgba {
        rgb(0x0d0e13)
    }
    pub fn border_subtle() -> Rgba {
        rgb(0x262833)
    }
    pub fn text() -> Rgba {
        rgb(0xd7dae0)
    }
    pub fn text_muted() -> Rgba {
        rgb(0x9da5b4)
    }
    pub fn text_bright() -> Rgba {
        rgb(0xffffff)
    }
    pub fn accent() -> Rgba {
        rgb(0x61afef)
    }
    pub fn ok() -> Rgba {
        rgb(0x98c379)
    }
    pub fn warn() -> Rgba {
        rgb(0xe5c07b)
    }
    pub fn danger() -> Rgba {
        rgb(0xef8891)
    }
    pub fn info() -> Rgba {
        rgb(0x56b6c2)
    }
    pub fn orange() -> Rgba {
        rgb(0xd19a66)
    }
    pub fn purple() -> Rgba {
        rgb(0xd08ae6)
    }
    pub fn on_accent() -> Rgba {
        rgb(0x1e2227)
    }

    /// A transparent background from a colour (for badges and banners).
    pub fn tint(color: Rgba, alpha: f32) -> Hsla {
        let mut hsla: Hsla = color.into();
        hsla.a = alpha;
        hsla
    }

    #[allow(dead_code)]
    pub fn scrim() -> Rgba {
        rgba(0x0c0e129e)
    }
}
