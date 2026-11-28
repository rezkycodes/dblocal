//! Embedded service icon SVG assets (Simple Icons 16.38.1, CC0-1.0) and brand tints.
//!
//! Valkey is not in Simple Icons and its upstream brand repository carries no
//! permissive asset licence (governed by LF Projects Trademark Policy), so
//! Valkey keeps its `VK` monogram badge.

use std::borrow::Cow;

use dblocal_core::model::EngineKey;
use dblocal_core::toolchain::ToolId;
use gpui_kit::{AssetSource, Hsla, SharedString, rgb};

const ICON_POSTGRESQL: &[u8] = include_bytes!("../assets/icons/postgresql.svg");
const ICON_MYSQL: &[u8] = include_bytes!("../assets/icons/mysql.svg");
const ICON_MARIADB: &[u8] = include_bytes!("../assets/icons/mariadb.svg");
const ICON_REDIS: &[u8] = include_bytes!("../assets/icons/redis.svg");
const ICON_RUSTFS: &[u8] = include_bytes!("../assets/icons/rustfs.svg");
const ICON_PHP: &[u8] = include_bytes!("../assets/icons/php.svg");
const ICON_NODE: &[u8] = include_bytes!("../assets/icons/node.svg");
const ICON_COMPOSER: &[u8] = include_bytes!("../assets/icons/composer.svg");

/// Asset source that serves DBLocal's embedded service SVGs alongside `gpui-kit`'s icons.
#[derive(Debug, Clone, Copy, Default)]
pub struct DblocalAssets;

impl AssetSource for DblocalAssets {
    fn load(&self, path: &str) -> gpui_kit::Result<Option<Cow<'static, [u8]>>> {
        let embedded = match path {
            "icons/services/postgresql.svg" => Some(ICON_POSTGRESQL),
            "icons/services/mysql.svg" => Some(ICON_MYSQL),
            "icons/services/mariadb.svg" => Some(ICON_MARIADB),
            "icons/services/redis.svg" => Some(ICON_REDIS),
            "icons/services/rustfs.svg" => Some(ICON_RUSTFS),
            "icons/toolchains/php.svg" => Some(ICON_PHP),
            "icons/toolchains/node.svg" => Some(ICON_NODE),
            "icons/toolchains/composer.svg" => Some(ICON_COMPOSER),
            _ => None,
        };
        if let Some(bytes) = embedded {
            return Ok(Some(Cow::Borrowed(bytes)));
        }
        gpui_kit::assets::AllAssets.load(path)
    }

    fn list(&self, path: &str) -> gpui_kit::Result<Vec<SharedString>> {
        if path == "icons/services" {
            return Ok(vec![
                SharedString::from("icons/services/postgresql.svg"),
                SharedString::from("icons/services/mysql.svg"),
                SharedString::from("icons/services/mariadb.svg"),
                SharedString::from("icons/services/redis.svg"),
                SharedString::from("icons/services/rustfs.svg"),
            ]);
        }
        if path == "icons/toolchains" {
            return Ok(vec![
                SharedString::from("icons/toolchains/php.svg"),
                SharedString::from("icons/toolchains/node.svg"),
                SharedString::from("icons/toolchains/composer.svg"),
            ]);
        }
        gpui_kit::assets::AllAssets.list(path)
    }
}

/// Asset path of an engine's service SVG, or `None` when the monogram badge is used.
pub const fn service_icon_path(engine: EngineKey) -> Option<&'static str> {
    match engine {
        EngineKey::Postgresql => Some("icons/services/postgresql.svg"),
        EngineKey::Mysql => Some("icons/services/mysql.svg"),
        EngineKey::Mariadb => Some("icons/services/mariadb.svg"),
        EngineKey::Redis => Some("icons/services/redis.svg"),
        EngineKey::Rustfs => Some("icons/services/rustfs.svg"),
        // Valkey keeps its VK letter badge.
        _ => None,
    }
}

/// Brand tint for an engine icon, lightened in dark mode where contrast is too low
/// (notably MariaDB `#003545`).
pub fn service_brand_colour(engine: EngineKey, is_dark: bool) -> Hsla {
    let hex: u32 = match (engine, is_dark) {
        (EngineKey::Postgresql, false) => 0x4169E1,
        (EngineKey::Postgresql, true) => 0x6C8EF5,
        (EngineKey::Mysql, false) => 0x4479A1,
        (EngineKey::Mysql, true) => 0x6BA3CC,
        (EngineKey::Mariadb, false) => 0x003545,
        (EngineKey::Mariadb, true) => 0x4FB3D9,
        (EngineKey::Redis, false) => 0xD63329,
        (EngineKey::Redis, true) => 0xFF6B61,
        (EngineKey::Rustfs, false) => 0x0196D0,
        (EngineKey::Rustfs, true) => 0x38BDF8,
        (_, false) => 0x6E56CF,
        (_, true) => 0xA78BFA,
    };
    rgb(hex).into()
}

/// Asset path of a toolchain's Simple Icons SVG.
pub const fn toolchain_icon_path(tool: ToolId) -> &'static str {
    match tool {
        ToolId::Php => "icons/toolchains/php.svg",
        ToolId::Node => "icons/toolchains/node.svg",
        ToolId::Composer => "icons/toolchains/composer.svg",
    }
}

/// Brand tint for a toolchain icon, lightened in dark mode so every mark stays
/// legible on dark chrome and card surfaces.
pub fn toolchain_brand_colour(tool: ToolId, is_dark: bool) -> Hsla {
    let hex: u32 = match (tool, is_dark) {
        (ToolId::Php, false) => 0x5E629C,
        (ToolId::Php, true) => 0x9CA2E6,
        (ToolId::Node, false) => 0x468038,
        (ToolId::Node, true) => 0x74C365,
        (ToolId::Composer, false) => 0x885630,
        (ToolId::Composer, true) => 0xD49A6A,
    };
    rgb(hex).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_all_five_service_svgs_and_keeps_valkey_on_badge() {
        for engine in [
            EngineKey::Postgresql,
            EngineKey::Mysql,
            EngineKey::Mariadb,
            EngineKey::Redis,
            EngineKey::Rustfs,
        ] {
            let path = service_icon_path(engine).expect("svg path");
            let bytes = DblocalAssets
                .load(path)
                .expect("load")
                .expect("embedded bytes");
            assert!(
                std::str::from_utf8(&bytes).expect("utf8").contains("<svg"),
                "{path} must be valid SVG"
            );
        }
        assert_eq!(service_icon_path(EngineKey::Valkey), None);
    }

    #[test]
    fn loads_all_three_toolchain_svgs_and_lightens_composer_in_dark() {
        for tool in ToolId::all() {
            let path = toolchain_icon_path(tool);
            let bytes = DblocalAssets
                .load(path)
                .expect("load")
                .expect("embedded bytes");
            assert!(
                std::str::from_utf8(&bytes).expect("utf8").contains("<svg"),
                "{path} must be valid SVG"
            );
        }
        let light = toolchain_brand_colour(ToolId::Composer, false);
        let dark = toolchain_brand_colour(ToolId::Composer, true);
        assert!(dark.l > light.l + 0.2, "dark={dark:?} light={light:?}");
    }

    #[test]
    fn dark_theme_lightens_mariadb_brand_colour() {
        let light = service_brand_colour(EngineKey::Mariadb, false);
        let dark = service_brand_colour(EngineKey::Mariadb, true);
        assert!(dark.l > light.l + 0.3, "dark={dark:?} light={light:?}");
    }

    /// The Delete actions draw the Lucide **trash** glyph, not `delete.svg`
    /// (the backspace symbol). `IconName::Trash` resolves to `icons/trash.svg`,
    /// and `DblocalAssets` must serve those bytes — including through the
    /// release path, where only the whole `AllAssets` catalog is embedded.
    ///
    /// A truncated or empty asset would render as a blank icon; asserting the
    /// `<svg>` root and the trash body proves bytes reach the renderer, and the
    /// generated `IconName::Trash` variant proves the name exists in the binary.
    #[test]
    fn the_trash_glyph_is_bundled_and_is_not_the_delete_glyph() {
        let trash = gpui_kit::assets::IconName::Trash;
        assert_eq!(trash.path().as_ref(), "icons/trash.svg");
        assert_ne!(
            trash.path().as_ref(),
            "icons/delete.svg",
            "delete.svg is the backspace glyph; DBLocal wants the trash can"
        );
        let bytes = DblocalAssets
            .load(trash.path().as_ref())
            .expect("load trash")
            .expect("trash.svg must be embedded, not blank");
        let svg = std::str::from_utf8(&bytes).expect("trash.svg is utf-8");
        assert!(svg.contains("<svg"), "trash.svg must be an SVG document");
        // The trash can body: a lid plus the bin outline Lucide's `trash` draws.
        assert!(
            svg.contains("M19 6v14"),
            "trash.svg must carry the bin body, not another glyph"
        );
    }
}
