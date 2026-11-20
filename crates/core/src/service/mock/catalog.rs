//! Catalog read-model construction for the mock.

use std::collections::BTreeSet;

use crate::model::{CatalogStatus, EngineKey};
use crate::platform::{Arch, Os};
use crate::service::{CatalogView, EngineEntry, VersionEntry};
use crate::strings;

/// One version entry the mock pretends the catalog knows about.
struct VersionRow {
    version: &'static str,
    eol: bool,
    size_bytes: u64,
}

fn rows(engine: EngineKey) -> Vec<VersionRow> {
    match engine {
        EngineKey::Postgresql => vec![
            VersionRow {
                version: "17.4",
                eol: false,
                size_bytes: 54_525_952,
            },
            VersionRow {
                version: "16.6",
                eol: false,
                size_bytes: 52_428_800,
            },
            VersionRow {
                version: "15.10",
                eol: true,
                size_bytes: 50_331_648,
            },
        ],
        EngineKey::Mysql => vec![
            VersionRow {
                version: "8.4.4",
                eol: false,
                size_bytes: 83_886_080,
            },
            VersionRow {
                version: "8.0.40",
                eol: false,
                size_bytes: 86_000_000,
            },
        ],
        EngineKey::Mariadb => vec![
            VersionRow {
                version: "11.4.2",
                eol: false,
                size_bytes: 73_400_320,
            },
            VersionRow {
                version: "10.11.10",
                eol: true,
                size_bytes: 71_303_168,
            },
        ],
        EngineKey::Valkey => vec![
            VersionRow {
                version: "8.1.0",
                eol: false,
                size_bytes: 14_680_064,
            },
            VersionRow {
                version: "7.2.8",
                eol: false,
                size_bytes: 12_582_912,
            },
        ],
    }
}

/// Why an engine cannot be installed here, if it cannot.
pub(crate) fn unavailable_reason(engine: EngineKey, os: Os, arch: Arch) -> Option<&'static str> {
    match engine {
        EngineKey::Mariadb if os == Os::Macos => Some(strings::CATALOG_ENGINE_UNAVAILABLE_PLATFORM),
        EngineKey::Mariadb if os == Os::Linux && arch == Arch::Aarch64 => {
            Some(strings::CATALOG_ENGINE_UNAVAILABLE_PLATFORM)
        }
        EngineKey::Valkey if os == Os::Windows => {
            Some(strings::CATALOG_ENGINE_UNAVAILABLE_PLATFORM)
        }
        _ => None,
    }
}

/// Builds the platform-filtered catalog read model.
pub(crate) fn view(
    status: CatalogStatus,
    os: Os,
    arch: Arch,
    installed: &BTreeSet<(EngineKey, String)>,
) -> CatalogView {
    let banner = match status {
        CatalogStatus::Fresh => None,
        CatalogStatus::Stale => Some(strings::CATALOG_STALE.to_owned()),
        CatalogStatus::Offline => Some(strings::CATALOG_OFFLINE.to_owned()),
    };
    let engines = EngineKey::ALL
        .into_iter()
        .map(|engine| {
            let reason = unavailable_reason(engine, os, arch);
            let versions = rows(engine)
                .into_iter()
                .map(|row| VersionEntry {
                    version: row.version.to_owned(),
                    eol: row.eol,
                    installed: installed.contains(&(engine, row.version.to_owned())),
                    size_bytes: row.size_bytes,
                })
                .collect();
            EngineEntry {
                engine,
                available: reason.is_none(),
                unavailable_reason: reason.map(str::to_owned),
                versions,
            }
        })
        .collect();
    CatalogView {
        status,
        banner,
        engines,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nothing_installed() -> BTreeSet<(EngineKey, String)> {
        BTreeSet::new()
    }

    #[test]
    fn fresh_catalog_has_no_banner() {
        let view = view(
            CatalogStatus::Fresh,
            Os::Linux,
            Arch::X86_64,
            &nothing_installed(),
        );
        assert!(view.banner.is_none());
        assert_eq!(view.engines.len(), 4);
    }

    #[test]
    fn stale_and_offline_carry_their_banner_text() {
        let stale = view(
            CatalogStatus::Stale,
            Os::Linux,
            Arch::X86_64,
            &nothing_installed(),
        );
        assert_eq!(stale.banner.as_deref(), Some(strings::CATALOG_STALE));
        let offline = view(
            CatalogStatus::Offline,
            Os::Linux,
            Arch::X86_64,
            &nothing_installed(),
        );
        assert_eq!(offline.banner.as_deref(), Some(strings::CATALOG_OFFLINE));
    }

    #[test]
    fn mariadb_is_disabled_on_macos_with_a_reason() {
        let view = view(
            CatalogStatus::Fresh,
            Os::Macos,
            Arch::Aarch64,
            &nothing_installed(),
        );
        let mariadb = view
            .engines
            .iter()
            .find(|e| e.engine == EngineKey::Mariadb)
            .expect("mariadb entry");
        assert!(!mariadb.available);
        assert_eq!(
            mariadb.unavailable_reason.as_deref(),
            Some(strings::CATALOG_ENGINE_UNAVAILABLE_PLATFORM)
        );
    }

    #[test]
    fn mariadb_is_disabled_on_linux_aarch64() {
        let view = view(
            CatalogStatus::Fresh,
            Os::Linux,
            Arch::Aarch64,
            &nothing_installed(),
        );
        let mariadb = view
            .engines
            .iter()
            .find(|e| e.engine == EngineKey::Mariadb)
            .expect("mariadb entry");
        assert!(!mariadb.available);
    }

    #[test]
    fn installed_versions_are_marked() {
        let mut installed = BTreeSet::new();
        installed.insert((EngineKey::Postgresql, "17.4".to_owned()));
        let view = view(CatalogStatus::Fresh, Os::Linux, Arch::X86_64, &installed);
        let pg = view
            .engines
            .iter()
            .find(|e| e.engine == EngineKey::Postgresql)
            .expect("pg");
        let v = pg
            .versions
            .iter()
            .find(|v| v.version == "17.4")
            .expect("17.4");
        assert!(v.installed);
        assert!(
            !pg.versions
                .iter()
                .find(|v| v.version == "16.6")
                .expect("16.6")
                .installed
        );
    }

    #[test]
    fn every_engine_has_at_least_one_version() {
        let view = view(
            CatalogStatus::Fresh,
            Os::Linux,
            Arch::X86_64,
            &nothing_installed(),
        );
        for entry in view.engines {
            assert!(
                !entry.versions.is_empty(),
                "{} has no versions",
                entry.engine
            );
        }
    }
}
