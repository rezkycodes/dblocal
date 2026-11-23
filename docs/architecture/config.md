# Config editor (F-CFG)

Core of the Config tab (`F-CFG-01`) and "open the raw file" (`F-CFG-02`). The GUI
tab (`W4-H`) renders this module's read model in `crate::config::view` and needs
nothing else from it.

## 1. Which file is the user's and which is DBLocal's?

DBLocal renders each engine's config once at `initialize` and rewrites only the
lines it manages afterwards; nothing regenerates the whole file on `start`, so a
user edit survives a restart. Verified per engine:

| Engine | File | Rendered at | Rewritten on `change_port` / start | Overwritten on start? |
|---|---|---|---|---|
| PostgreSQL | `<data>/postgresql.conf` | `configure_after_init` appends a fenced `# DBLocal managed settings` block after `initdb` wrote its base config | `sync_port` replaces that block (or, in a legacy layout, the managed lines) | No. `start` never writes the file. |
| MySQL | `<run>/my.cnf` | `write_my_cnf` in `initialize` | `sync_port` rewrites `port=`/`socket=` | No. `start` only `prepare_run`s directories. |
| MariaDB | `<data>/my.cnf` | `render_config` + `write_private_config` in `initialize` | `sync_port` rewrites `port=`/`socket=` | No. |
| Valkey | `<run>/valkey.conf` | `render_config` in `initialize` (carries `requirepass` in password mode) | `sync_port` rewrites the `port` line | No. |

No adapter regenerates the file on `start`. The one caveat is that `initialize`
writes a fresh managed block / config, so the user's file is only authored from a
blank slate once; after that DBLocal edits lines in place.

## 2. How a user edit coexists with the managed lines

The rule is PRD `F-CFG-02` AC1: "komentar serta baris yang tidak dikenal app tidak
pernah ditulis ulang atau dihapus". So the editor is **line-level**, not
parse-and-re-render (`crate::config::editor`):

- it changes only the line of the key it was asked to change;
- "the last uncommented assignment wins, as the engines read it": an update
  rewrites the **last** matching line and leaves earlier duplicates alone;
- a missing key is appended as `key<sep>value`: `key = value` for PostgreSQL,
  `key=value` inside `[mysqld]` for MySQL/MariaDB, `key value` for Valkey;
- it never reorders, never touches a comment, an unknown key, a blank line, the
  line-ending style (CRLF stays CRLF, including on the appended line) or a
  trailing-newline-less last line when it edits rather than appends;
- for INI, an appended key lands under the last setting of `[mysqld]`, before the
  blank line that separates it from the next section; an `!include` line is kept.

**Managed keys are not editable here** and are refused with `DBL-8602`: `port`,
`socket`, `tmpdir`, log/pid paths, `datadir`, `basedir`, `bind`, `bind-address`,
`listen_addresses`, `unix_socket_directories`, `requirepass`, `dir`,
`data_directory`, `pid-file`, `pidfile`, `log`, `log-error`, `logfile`. `port`
goes through the existing `change_port`.

## 3. Defaults and units

Defaults are read from the real engine, not memory. The version each table was
checked against and the command that produced it are in
`crate::config::schema::DEFAULTS_PROVENANCE`; `tests/config_engine_e2e.rs`
(`#[ignore]`) re-checks them so a catalog bump that changes a default fails the
e2e.

| Engine (version) | Command | Keys checked |
|---|---|---|
| PostgreSQL 18.7.0 | `postgres -D <initdb dir> -C <key>` after `initdb -A trust` | `max_connections` 100, `shared_buffers` 16384 (128MB), `work_mem` 4096 (4MB), `TimeZone` GMT |
| MySQL 8.4.11 | `mysqld --no-defaults --verbose --help` | `max-connections` 151, `innodb-buffer-pool-size` 134217728, `sort-buffer-size` 262144, `character-set-server` utf8mb4, `default-time-zone` (none) |
| MariaDB 11.8.10 | `mariadbd --no-defaults --verbose --help` (with the libcrypt shim) | same, but `sort-buffer-size` 2097152 (2MB) |
| Valkey 9.2.0 | `valkey-server` then `CONFIG GET maxmemory maxmemory-policy appendonly save timeout` | `maxmemory` 0, `maxmemory-policy` noeviction, `appendonly` no, `save` "3600 1 300 100 60 10000", `timeout` 0 |

Every key in this subset needs a restart (`ApplyTime::Restart`), and the form
says so per key.

Time zones are validated against the system tzdata (`/usr/share/zoneinfo`), the
same source the engines resolve a zone name with — no time-zone crate is added.
Where that tree is absent (Windows, a minimal container) only the `Area/Location`
shape and a UTC offset are checked, and the engine refuses an unknown name at
start, which is exactly the `F-CFG-01` AC3 restore path.

## 4. Read-only: authentication method

`F-CFG-01` AC1 lists "metode autentikasi" in the form. This task shows the
current PostgreSQL method (the methods of the `pg_hba.conf` `host` lines DBLocal
manages) but **does not edit `pg_hba.conf`**: a bad edit can lock the user out of
an instance whose credential DBLocal holds. This is a deliberate deviation,
reported to the reviewer.

## 5. Backups, restore and "restore available"

Layout under the DBLocal home (`crate::config::backup`):

```text
<home>/dblocal-backups/<instance>/<file>.<unix-ts>.bak   0600, keep the newest 5
<home>/dblocal-backups/<instance>/<file>.last-good        0600, sha-256 of the
                                                          file at the last Running
```

The directory is `0700`; the files are `0600`, because `valkey.conf` may hold
`requirepass` and a backup must be exactly as secret as the file. Nothing reads a
backup into a `String` that could be logged: it is copied file-to-file.

`config_apply` takes one backup of the file as it is now, then performs one
atomic write in which every edit is applied to the same in-memory text. Every key
and value is validated **before** the file is touched; one refused entry refuses
the whole call.

`config_restore` puts the newest backup back atomically and first backs up the
file it replaces, so a restore is itself undoable.

When `start_instance` fails and the file changed since the last successful start
(recorded hash ≠ current hash, with a backup present), the failure carries
`restore_available = true`. "Last successful start" is a recorded hash, not an
mtime.

## 6. Guarding against a lifecycle operation in flight

`config_apply` and `config_restore` take the per-instance guard
(`Ops::lock_instance`), so a `start`/`stop`/`delete` in flight refuses the call as
`DBL-8604` (busy) instead of interleaving with it. A running instance is allowed:
the change is for the next start.

## 7. Opening the raw file

`config_raw_path(id)` returns the path and `config_open_raw(id)` opens it in the
OS default handler (`F-CFG-02` AC1). The argv is a pure function of the OS and
the path (`config::launcher`): `xdg-open <path>` on Linux, `open <path>` on macOS,
`rundll32.exe url.dll,FileProtocolHandler <path>` on Windows — never
`cmd /c start`, whose parser would see `&`, `^`, `%` and quotes in the path. The
path is exactly one argv element, so a space, `&`, `%` or a leading `-` is never a
second argument (a leading `-` is prefixed with `./` so `xdg-open` cannot read it
as an option). The launch is detached, argv only, no shell, through an
`OpenLauncher` seam a test injects a fake into; the real service uses
`SystemOpenLauncher`. A missing file is `DBL-8607`; a handler that cannot start is
`DBL-8608`, and the failure prints the path so the user can open it by hand. The
CLI command is `dblocal config open <name>`. Nothing here reads or rewrites the
file, so no content can leak.

## 8. Not verified here

- Windows/macOS file semantics: the CRLF fixtures cover the parser only.
- The Windows (`rundll32`) and macOS (`open`) launchers were not run: only the
  Linux `xdg-open` path was exercised with a stub that records its argv.
- The OS default editor actually opening a window (a human must click it).
- MySQL 8.0/9.x and MariaDB 11.4: this machine has MySQL 8.4.11 and MariaDB
  11.8.10; 11.4 needs the libcrypt shim and is a printed skip elsewhere.
