//! Schema DDL, migration list and store constants.

/// How many times the `journal_mode=WAL` switch is retried when a competing
/// opener holds the database lock.
pub(crate) const WAL_SWITCH_ATTEMPTS: u32 = 40;

/// Pause between WAL switch retries, in milliseconds.
pub(crate) const WAL_SWITCH_BACKOFF_MS: u64 = 25;

/// Latest schema version this binary understands.
pub(crate) const LATEST_USER_VERSION: i32 = 1;

/// Ordered migrations. Index `n` upgrades schema version `n` to `n + 1`.
pub(crate) const MIGRATIONS: &[&str] = &[MIGRATION_V1];

/// Version 1: instances, runtimes, archives, credential references, settings.
///
/// No column can hold a password, token or connection URI (AGENTS.md invariant
/// 5). `credentials` stores only a pointer into the OS keychain or the `0600`
/// file fallback, and it cascades away with its instance.
const MIGRATION_V1: &str = r#"
CREATE TABLE instances (
    id                       TEXT PRIMARY KEY NOT NULL,
    name                     TEXT NOT NULL COLLATE NOCASE UNIQUE,
    engine                   TEXT NOT NULL,
    version                  TEXT NOT NULL,
    port                     INTEGER NOT NULL UNIQUE,
    status                   TEXT NOT NULL,
    auth                     TEXT NOT NULL,
    superuser                TEXT NOT NULL,
    pid                      INTEGER,
    process_started_at_secs  INTEGER,
    process_started_at_nanos INTEGER,
    process_exe              TEXT,
    autostart                INTEGER NOT NULL,
    created_at_secs          INTEGER NOT NULL,
    created_at_nanos         INTEGER NOT NULL,
    CHECK (autostart IN (0, 1)),
    CHECK (port BETWEEN 1024 AND 65535),
    CHECK (created_at_nanos BETWEEN 0 AND 999999999),
    CHECK (
        (pid IS NULL AND process_started_at_secs IS NULL
            AND process_started_at_nanos IS NULL AND process_exe IS NULL)
        OR
        (pid IS NOT NULL AND process_started_at_secs IS NOT NULL
            AND process_started_at_nanos IS NOT NULL AND process_exe IS NOT NULL)
    )
);

CREATE TABLE runtimes (
    id                 TEXT PRIMARY KEY NOT NULL,
    engine             TEXT NOT NULL,
    version            TEXT NOT NULL,
    arch               TEXT NOT NULL,
    path               TEXT NOT NULL,
    size_bytes         INTEGER NOT NULL,
    state              TEXT NOT NULL,
    installed_at_secs  INTEGER NOT NULL,
    installed_at_nanos INTEGER NOT NULL,
    CHECK (size_bytes >= 0),
    CHECK (installed_at_nanos BETWEEN 0 AND 999999999),
    UNIQUE (engine, version, arch)
);

CREATE TABLE archives (
    id                TEXT PRIMARY KEY NOT NULL,
    instance_name     TEXT NOT NULL,
    engine            TEXT NOT NULL,
    version           TEXT NOT NULL,
    path              TEXT NOT NULL,
    size_bytes        INTEGER NOT NULL,
    archived_at_secs  INTEGER NOT NULL,
    archived_at_nanos INTEGER NOT NULL,
    CHECK (size_bytes >= 0),
    CHECK (archived_at_nanos BETWEEN 0 AND 999999999)
);

CREATE TABLE credentials (
    instance_id TEXT PRIMARY KEY NOT NULL REFERENCES instances(id) ON DELETE CASCADE,
    backend     TEXT NOT NULL,
    key_ref     TEXT NOT NULL,
    username    TEXT NOT NULL
);

CREATE TABLE settings (
    key   TEXT PRIMARY KEY NOT NULL,
    value TEXT NOT NULL
);
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migration_count_matches_latest_version() {
        assert_eq!(MIGRATIONS.len() as i32, LATEST_USER_VERSION);
    }
}
