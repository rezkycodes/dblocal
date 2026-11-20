# W1-A — SQLite store

Branch: `w1-store`. Read first: AGENTS.md, `docs/architecture/CONTRACT.md`, the `entities` and `decisions` (D-11, D-17, D-18) in
`docs/prd/dblocal/feature-map.json`, PRD §6 `F-INS-*`, `F-RTM-04`, `F-CRD-01`.

## Goal

A synchronous, well-tested persistence layer for instances, installed runtimes, archived data and settings. The service layer
(W2-A) will call it from `spawn_blocking`; you never touch async.

## You own

`crates/core/src/store.rs`, `crates/core/src/store/**`, `docs/architecture/store.md`. Nothing else. If you need a change to a model
type (`model/entity.rs`, `model/runtime.rs`), use `ask`; do not edit it.

## Deliverables

- `Store::open(&DblPaths)`: creates the directory (mode `0700` on Unix) and the database file, then sets
  `journal_mode=WAL`, `foreign_keys=ON`, `synchronous=NORMAL`, `busy_timeout` (use a const). Also `Store::open_in_memory()` for tests.
- Versioned migrations driven by `PRAGMA user_version`: ordered list of `&'static str` steps, each run in one transaction, idempotent,
  refuses a database newer than the binary (typed error with a stable `DBL-` code, never a panic).
- Schema for: `instances` (every field of `model::Instance`; name unique case-insensitively; `port` unique), `runtimes` (installed engine
  builds: engine, version, path, size, state, installed_at), `archives` (data kept by "archive" delete, D-18), `settings` (key/value).
- **No password, secret or connection URI column anywhere** (AGENTS.md invariant 5). Store only a `CredentialRef`. A test must scan
  `sqlite_master` and fail if any column name matches `pass|secret|token|uri`.
- Typed API (no SQL leaks out): insert / get / get-by-name / list / update-status / update-process-identity (`pid`, `process_started_at`,
  `process_exe`) / update-port / set-autostart / delete instance; runtime add / list / remove / total-size; archive add / list / remove;
  setting get / set. Every write that touches more than one row is one transaction.
- `update_status` persists what it is given; transition rules live in `InstanceStatus::apply` and are the service's job. Do not duplicate them.
- Corrupt or unreadable database: return a typed error and leave the file untouched (never delete user data).

## Tests (write first, RED then GREEN)

Migration from empty; re-open is a no-op; a hand-built "future" `user_version` is refused; unique name (case-insensitive) and unique port
violations map to typed errors; delete cascades what it should and nothing else; two connections, one writing while the other reads (WAL);
crash-safety (drop without commit leaves no partial row); the no-secret-column test above; a `DBLOCAL_HOME` temp directory is used, never `~/.dblocal`.

## Done when

`cargo xtask ci` is green on your branch, `cargo llvm-cov -p dblocal-core` shows >= 80% lines for each file you own, and your report lists the
schema as a table, every public function, and anything you could not verify.
