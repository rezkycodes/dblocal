# AGENTS.md — DBLocal worker contract

DBLocal is a Rust desktop app (GPUI) that installs and manages local PostgreSQL, MySQL, MariaDB and
Valkey servers, several versions side by side, with no containers and no sudo.
License: **GPL-3.0-only**.

**Source of truth:** `docs/prd/dblocal/PRD.md` and `docs/prd/dblocal/feature-map.json`.
If this file and the PRD disagree, the PRD wins, except for the "PRD corrections" below.

## 1. How you work here

- You are one worker in a multi-agent build coordinated through Orca. The coordinator reviews every
  result against the PRD acceptance criteria. A "done" claim without command output is rejected.
- You run in **your own git worktree on your own branch**. Commit there (small conventional commits).
  Do **not** push, merge, rebase onto other branches, or touch the main worktree.
- Edit **only** the paths under "You own" in your task file. If you need a change anywhere else
  (another worker's module, shared Cargo.toml, the contract), do not make it: use Orca `ask`
  (see your dispatch preamble) or list it in your final report.
- Read in this order: this file → your task file in `docs/orchestration/tasks/` → the PRD sections
  the task cites.
- Language: code, identifiers, comments, commit messages and docs in English. User-facing UI strings
  are Indonesian exactly as the PRD writes them, and live only in a `strings` module.

## 2. Environment (this machine)

- Rust **1.95.0** stable, edition 2024. No sudo, no `dnf`/`apt`, no global installs. If a system
  library is missing, stop and `ask`; do not work around it by vendoring a C library silently.
  Known missing dev packages: `libsecret-1`, `alsa`. Do not add dependencies that need them.
- Cargo target dirs (shared across worktrees so nobody recompiles dependencies from scratch):
  - non-GUI work: `export CARGO_TARGET_DIR="$HOME/.cache/dblocal-target"`
  - anything that compiles `gpui`: `export CARGO_TARGET_DIR="$HOME/.cache/dblocal-target-gui"`
  A "Blocking waiting for file lock" message is normal. Wait; do not delete lock files.
- Network is allowed for crates and for the engine downloads a task names. Keep downloaded archives in
  `$HOME/.cache/dblocal-dev/`, never inside the repo.
- The crate versions below are newer than your training data. Verify APIs in `~/.cargo/registry`
  sources or docs.rs, not from memory.
- Installed dev tools: `cargo-deny`, `cargo-llvm-cov` (may still be compiling; check `cargo deny --version`).

### Pinned versions (verified 2026-11-20)

| Crate | Version | Note |
|---|---|---|
| gpui | `{ package = "gpui-pre", version = "=0.3.12" }` | PRD correction: `gpui-component 0.10.0` hard-requires `gpui-pre =0.3.12`; 0.3.13 will not resolve |
| gpui-component | `=0.10.0` | Apache-2.0 |
| tokio 1.54 · reqwest 0.13 (rustls, no default features) · rusqlite 0.43 (`bundled`) · sha2 0.11 · clap 4.6 | | |
| thiserror 2 · anyhow 1 · serde 1 · serde_json 1 · tracing 0.1 · tracing-subscriber 0.3 | | |
| directories 6 · semver 1 · url 2 · percent-encoding 2 · rand 0.10 · tempfile 3 · which 8 | | |
| tar 0.4 · flate2 1.1 · liblzma 0.4 (not xz2) · zip 9 | | |
| sysinfo 0.40 · nix 0.32 (unix) · windows-sys 0.61 (windows) · keyring 4.2 | | keyring backend must build without libsecret-1 |
| tokio-postgres 0.7 · mysql_async 0.37 · redis 1.8 | | health checks and admin ops |
| tray-icon 0.26 | | MSRV 1.90 |

### PRD corrections

1. Pin `gpui-pre =0.3.12` and `gpui-component =0.10.0` (see table).
2. One binary named `dblocal`, built by `crates/cli`. Without arguments it opens the GUI
   (`dblocal_gui::run()`); with a subcommand it runs the CLI. `crates/gui` is a library.
3. The UI is **Zed-style** (decision D-23, supersedes the DBDelve reference): docks, a tabbed centre pane, a status bar,
   a command palette. Read `docs/design/UI-DIRECTION.md` before touching any GUI code. Contrast of at least 4.5:1 is enforced
   by a test; Zed's own muted greys fail it and must be lightened. Zed's `ui` and `theme` crates are GPL-3.0-or-later and not
   dependencies; write our own code and treat the Apache-2.0 GPUI examples as an API reference.

## 3. Hard invariants (PRD §9, §10). Violating any of these fails review.

1. Engines listen on `127.0.0.1` / `::1` only. Never `0.0.0.0`.
2. Never run or request `sudo`, `pkexec` or any elevation. Never write outside `~/.dblocal`
   (tests use a temp dir via `DBLOCAL_HOME`).
3. Every downloaded byte is SHA-256 verified before extraction or execution. No skip flag.
4. Archive extraction rejects `..`, absolute paths, and symlinks that escape the destination.
5. Passwords never appear in SQLite, logs, error messages, process argv, or `Debug` output.
   Use the `SecretString` newtype.
6. TLS is rustls only. No `openssl` / `native-tls` crates.
7. Engine processes are spawned detached. Before signalling a PID, verify executable name **and**
   process start time. Never signal a process you cannot positively identify.
8. No telemetry. Network hosts are limited to catalog sources and the GitHub API for updates.
9. Tests never bind 5432, 3306, 3307, 6379 or any port below 20000. Allocate with `bind(:0)`, or use
   40000-49999. Real engine processes appear only in `#[ignore]`d e2e tests and spikes, and are always
   stopped and cleaned up.

## 4. Code quality (hard rules)

- **Immutability.** Domain types are immutable value types: no `pub` mutable fields, updates return a
  new value (`with_*`). `&mut` is fine for I/O handles and local accumulation, not for domain state.
- **Size.** Files 200-400 lines typical, 800 hard maximum. Functions under 50 lines. Nesting at most 4.
- **Errors.** No `unwrap()`, `expect()`, `panic!`, `todo!`, `unimplemented!` outside `#[cfg(test)]`.
  Return `DblError` (stable `DBL-xxxx` code + friendly message). Never swallow an error. Log detail
  with `tracing`, never secrets.
- **Boundaries.** Validate at every boundary: names, ports, URLs, archive paths, catalog JSON.
- **No hardcoded values.** Ports, timeouts, sizes and paths live in `consts.rs` / `paths.rs`.
- **Unsafe.** `unsafe_code = "deny"` workspace-wide. Only the process module may opt in, per item, with a
  `// SAFETY:` comment.
- **TDD.** Write the failing test first, then the code. At least **80% line coverage** on the files you own.
- **Tests offline by default.** `cargo test` must pass with no internet and no real engines. Use local
  test servers (wiremock or a tiny tokio TCP server). Network or real-engine tests are
  `#[ignore = "e2e: ..."]` with the exact run command in the module docs.

## 5. Quality gates (run before reporting done)

```sh
export CARGO_TARGET_DIR="$HOME/.cache/dblocal-target"
cargo fmt --all -- --check
cargo clippy -p <your crates> --all-targets -- -D warnings
cargo test -p <your crates>
cargo llvm-cov -p <your crate> --summary-only        # coverage of files you own
cargo deny check licenses bans                       # whenever Cargo.lock changed
```

## 6. Final report (your `worker_done` message must contain all of these)

1. Summary, 10 lines at most.
2. Branch name, last commit hash, and `git diff --stat main...HEAD`.
3. Every quality gate command with pass/fail and the key output lines (paste, do not paraphrase).
4. Coverage percentage for the files you own.
5. Deviations from the task, PRD or contract, and why.
6. Open issues and anything the reviewer should double-check.

If a gate fails, report the failure with its output. Never report done over a red gate.
