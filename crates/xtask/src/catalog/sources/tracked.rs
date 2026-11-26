//! The version lines the DBLocal catalog offers, in one declarative table.
//!
//! Each entry says what the picker should show: the tag, the release line it
//! groups under, whether upstream still supports it, and the support status
//! (`lts`, `innovation`, `rolling`). Every line's newest patch is listed; the
//! picker hides older patches unless "Only show latest versions" is unchecked, so the
//! table only needs to say which patches exist for the "all" list.
//!
//! Adding or retiring a version is a one-line edit here, and the daily catalog
//! refresh picks it up the next run.

use dblocal_core::model::{EngineKey, VersionStatus};

/// One tracked version: a tag plus the picker metadata for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrackedVersion {
    /// Exact upstream tag, for example `18.7.0`.
    pub version: &'static str,
    /// Release line the picker groups by, for example `18` or `8.4`.
    pub line: &'static str,
    /// Upstream no longer supports the line.
    pub eol: bool,
    /// Advertised support status, if the vendor names one.
    pub status: Option<VersionStatus>,
}

const fn v(version: &'static str, line: &'static str) -> TrackedVersion {
    TrackedVersion {
        version,
        line,
        eol: false,
        status: None,
    }
}

const fn lts(version: &'static str, line: &'static str) -> TrackedVersion {
    TrackedVersion {
        version,
        line,
        eol: false,
        status: Some(VersionStatus::Lts),
    }
}

const fn innovation(version: &'static str, line: &'static str) -> TrackedVersion {
    TrackedVersion {
        version,
        line,
        eol: false,
        status: Some(VersionStatus::Innovation),
    }
}

const fn rolling(version: &'static str, line: &'static str) -> TrackedVersion {
    TrackedVersion {
        version,
        line,
        eol: false,
        status: Some(VersionStatus::Rolling),
    }
}

const fn eol(version: &'static str, line: &'static str) -> TrackedVersion {
    TrackedVersion {
        version,
        line,
        eol: true,
        status: None,
    }
}

/// PostgreSQL: every supported line plus the just-EOL 14.
///
/// 19.0 is the new major; 14.25 is the final 14 release (EOL 2026-11-12). 15,
/// 16, 17 and 18 stay on their newest patch. Two patches per line are listed so
/// the "all versions" toggle has something to show without downloading a whole
/// line's history.
pub const POSTGRESQL: &[TrackedVersion] = &[
    v("19.0.0", "19"),
    v("18.7.0", "18"),
    v("18.6.0", "18"),
    v("17.12.0", "17"),
    v("17.11.0", "17"),
    v("16.16.0", "16"),
    v("16.15.0", "16"),
    v("15.20.0", "15"),
    v("15.19.0", "15"),
    eol("14.25.0", "14"),
];

/// MariaDB: LTS 12.3/11.8/11.4/10.11 plus the rolling 13.1 stable.
///
/// 13.2 is a release candidate and is deliberately not offered. 10.11 is LTS but
/// its newest patch is the only one listed.
pub const MARIADB: &[TrackedVersion] = &[
    rolling("13.1.2", "13.1"),
    lts("12.3.4", "12.3"),
    lts("11.8.10", "11.8"),
    lts("11.4.14", "11.4"),
    lts("10.11.20", "10.11"),
];

/// MySQL: 8.4 and 9.7 LTS, the 26.10 Innovation line, and EOL 8.0.
///
/// No 9.x Innovation release is offered. 8.0 is EOL (2026-04-30) and stays with
/// its badge. The 26.10 tag uses the new two-part year numbering and has no
/// macOS x86_64 build; see [`MYSQL_BUILDS`].
pub const MYSQL: &[TrackedVersion] = &[
    innovation("26.10.0", "26.10"),
    lts("9.7.4", "9.7"),
    lts("8.4.13", "8.4"),
    eol("8.0.46", "8.0"),
];

/// Valkey: the current 9.2 plus the 8.1, 8.0 and 7.2 lines.
pub const VALKEY: &[TrackedVersion] = &[
    v("9.2.0", "9.2"),
    v("8.1.12", "8.1"),
    v("8.0.13", "8.0"),
    v("7.2.16", "7.2"),
];

/// Redis: 7.4.12 only (Linux `jammy` APT package; 7.0 is EOL and not offered).
pub const REDIS: &[TrackedVersion] = &[v("7.4.12", "7.4")];

/// RustFS: latest stable >= 1.0.6 resolved from GitHub releases (`rustfs/rustfs`).
pub const RUSTFS: &[TrackedVersion] = &[v("1.0.7", "1.0")];

/// The tracked table for one engine.
pub fn for_engine(engine: EngineKey) -> &'static [TrackedVersion] {
    match engine {
        EngineKey::Postgresql => POSTGRESQL,
        EngineKey::Mysql => MYSQL,
        EngineKey::Mariadb => MARIADB,
        EngineKey::Valkey => VALKEY,
        EngineKey::Redis => REDIS,
        EngineKey::Rustfs => RUSTFS,
        // `EngineKey` is `#[non_exhaustive]`; a new engine gets its own table.
        _ => &[],
    }
}

/// The entry for `version` inside a tracked table, if that exact tag is listed.
///
/// Test-only: the production build always takes the whole table.
#[cfg(test)]
pub fn find<'a>(table: &'a [TrackedVersion], version: &str) -> Option<&'a TrackedVersion> {
    table.iter().find(|entry| entry.version == version)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn every_table_is_newest_first_per_line_and_unique() {
        for engine in EngineKey::ALL {
            let table = for_engine(engine);
            let mut seen = BTreeSet::new();
            for entry in table {
                assert!(
                    seen.insert(entry.version),
                    "{} lists {} twice",
                    engine.as_str(),
                    entry.version
                );
            }
            // The picker's grouping relies on a line never reopening after it
            // closed, so the table is ordered by line, newest line first.
            let mut seen_lines = BTreeSet::new();
            let mut last_line = None;
            for entry in table {
                if Some(entry.line) != last_line {
                    assert!(
                        seen_lines.insert(entry.line),
                        "{} reopens line {} out of order",
                        engine.as_str(),
                        entry.line
                    );
                    last_line = Some(entry.line);
                }
            }
        }
    }

    #[test]
    fn lines_match_the_engine_tag_shape() {
        for engine in EngineKey::ALL {
            for entry in for_engine(engine) {
                assert_eq!(
                    dblocal_core::model::version_line(engine, entry.version),
                    entry.line,
                    "{} {} line",
                    engine.as_str(),
                    entry.version
                );
            }
        }
    }

    #[test]
    fn the_owners_versions_are_all_present() {
        // Owner decisions, 2026-11-26. A missing one fails the build here rather
        // than in review.
        for (engine, tags) in [
            (
                EngineKey::Postgresql,
                &[
                    "19.0.0", "18.7.0", "17.12.0", "16.16.0", "15.20.0", "14.25.0",
                ][..],
            ),
            (
                EngineKey::Mariadb,
                &["13.1.2", "12.3.4", "11.8.10", "11.4.14", "10.11.20"][..],
            ),
            (
                EngineKey::Mysql,
                &["26.10.0", "9.7.4", "8.4.13", "8.0.46"][..],
            ),
            (
                EngineKey::Valkey,
                &["9.2.0", "8.1.12", "8.0.13", "7.2.16"][..],
            ),
            (EngineKey::Redis, &["7.4.12"][..]),
            (EngineKey::Rustfs, &["1.0.7"][..]),
        ] {
            let table = for_engine(engine);
            for tag in tags {
                assert!(
                    find(table, tag).is_some(),
                    "{} is missing tracked version {tag}",
                    engine.as_str()
                );
            }
        }
    }

    #[test]
    fn eol_and_status_flags_match_the_decisions() {
        let pg14 = find(POSTGRESQL, "14.25.0").expect("pg 14.25");
        assert!(pg14.eol);
        let mysql80 = find(MYSQL, "8.0.46").expect("mysql 8.0.46");
        assert!(mysql80.eol);
        let mysql2610 = find(MYSQL, "26.10.0").expect("mysql 26.10");
        assert_eq!(mysql2610.status, Some(VersionStatus::Innovation));
        let mysql97 = find(MYSQL, "9.7.4").expect("mysql 9.7.4");
        assert_eq!(mysql97.status, Some(VersionStatus::Lts));
        let mariadb131 = find(MARIADB, "13.1.2").expect("mariadb 13.1.2");
        assert_eq!(mariadb131.status, Some(VersionStatus::Rolling));
        let mariadb123 = find(MARIADB, "12.3.4").expect("mariadb 12.3.4");
        assert_eq!(mariadb123.status, Some(VersionStatus::Lts));
        // The release candidate is never tracked.
        assert!(find(MARIADB, "13.2.1").is_none());
    }
}
