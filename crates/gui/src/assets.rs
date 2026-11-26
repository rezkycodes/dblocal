//! Embedded service icon SVG assets (Simple Icons 16.38.1, CC0-1.0) and brand tints.
//!
//! Valkey is not in Simple Icons and its upstream brand repository carries no
//! permissive asset licence (governed by LF Projects Trademark Policy), so
//! Valkey keeps its `VK` monogram badge.

use std::borrow::Cow;

use dblocal_core::model::EngineKey;
use gpui_kit::{AssetSource, Hsla, SharedString, rgb};

const ICON_POSTGRESQL: &[u8] = include_bytes!("../assets/icons/postgresql.svg");
const ICON_MYSQL: &[u8] = include_bytes!("../assets/icons/mysql.svg");
const ICON_MARIADB: &[u8] = include_bytes!("../assets/icons/mariadb.svg");
const ICON_REDIS: &[u8] = include_bytes!("../assets/icons/redis.svg");
const ICON_RUSTFS: &[u8] = include_bytes!("../assets/icons/rustfs.svg");

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
    fn dark_theme_lightens_mariadb_brand_colour() {
        let light = service_brand_colour(EngineKey::Mariadb, false);
        let dark = service_brand_colour(EngineKey::Mariadb, true);
        assert!(dark.l > light.l + 0.3, "dark={dark:?} light={light:?}");
    }
}
