# W0-A Report — Workspace skeleton and core contract

## 1. Summary
- Created compile-clean Cargo workspace (`core`, `gui`, `cli`, `xtask`) with pinned toolchain/lints and shared deps.
- Implemented tested core contract: errors (`DBL-NNNN`), consts, secret, platform, paths, model/state machine, engine trait + stubs, `DblService` trait/events/views.
- Delivered CLI/xtask stubs and `docs/architecture/CONTRACT.md` for Wave 1 workers.

## 2. Branch and diff
- Branch: `rezkycodes/w0-skeleton`
- Last code commit: `e147b80` (`chore(workspace): declare gpui and tray-icon pins for wave 1 gui`)
- Commits on branch: `0ade3a0` (skeleton + contract), `dd711f2` (CONTRACT.md), `edd4aa7` (tests), `a17d00b` (engine signature + cli tests), `e147b80` (gui pins), then this report commit
- `git diff --stat main...HEAD`:
```
 .cargo/config.toml                 |    2 +
 Cargo.lock                         | 3399 ++++++++++++++++++++++++++++++++++++
 Cargo.toml                         |   65 +
 clippy.toml                        |    4 +
 crates/cli/Cargo.toml              |   19 +
 crates/cli/src/cli.rs              |  280 +++
 crates/cli/src/main.rs             |   12 +
 crates/core/Cargo.toml             |   46 +
 crates/core/src/catalog.rs         |    1 +
 crates/core/src/consts.rs          |   81 +
 crates/core/src/download.rs        |    1 +
 crates/core/src/engine/mariadb.rs  |    1 +
 crates/core/src/engine/mod.rs      |  252 +++
 crates/core/src/engine/mysql.rs    |    1 +
 crates/core/src/engine/postgres.rs |    1 +
 crates/core/src/engine/valkey.rs   |    1 +
 crates/core/src/error.rs           |  318 ++++
 crates/core/src/instance.rs        |    1 +
 crates/core/src/lib.rs             |   20 +
 crates/core/src/model/artifacts.rs |  763 ++++++++
 crates/core/src/model/entity.rs    |  603 +++++++
 crates/core/src/model/mod.rs       |   68 +
 crates/core/src/model/scalar.rs    |  391 +++++
 crates/core/src/model/state.rs     |  334 ++++
 crates/core/src/paths.rs           |  292 ++++
 crates/core/src/platform.rs        |  188 ++
 crates/core/src/ports.rs           |    1 +
 crates/core/src/process.rs         |    1 +
 crates/core/src/secret.rs          |   96 +
 crates/core/src/secrets.rs         |    1 +
 crates/core/src/service/mock.rs    |    1 +
 crates/core/src/service/mod.rs     |  251 +++
 crates/core/src/store.rs           |    1 +
 crates/core/src/strings.rs         |   65 +
 crates/gui/Cargo.toml              |   13 +
 crates/gui/src/lib.rs              |   11 +
 crates/xtask/Cargo.toml            |   12 +
 crates/xtask/src/main.rs           |   80 +
 docs/architecture/CONTRACT.md      |  132 ++
 rust-toolchain.toml                |    3 +
 rustfmt.toml                       |    1 +
 41 files changed, 7813 insertions(+)
```

## 3. Quality gates (pass/fail + key output)

- `cargo build --workspace` (CARGO_TARGET_DIR="$HOME/.cache/dblocal-target"): **PASS**
  - `Finished \`dev\` profile [unoptimized + debuginfo] target(s) in 3.30s`
- `cargo xtask ci`: **PASS**
  - `==> cargo fmt --all -- --check`
  - `==> cargo clippy --workspace --all-targets -- -D warnings`
  - `==> cargo test --workspace`
  - `test result: ok. 8 passed; 0 failed; 0 ignored` (dblocal-cli)
  - `test result: ok. 52 passed; 0 failed; 0 ignored` (dblocal-core lib)
  - `test result: ok. 0 passed; 0 failed; 0 ignored` (dblocal-gui, xtask, doc-tests)
  - `xtask ci: all gates passed`
- `cargo fmt --all -- --check`: **PASS** (exit 0)
- `cargo clippy -p dblocal-core -p dblocal-gui -p dblocal-cli -p xtask --all-targets -- -D warnings`: **PASS** (covered by xtask ci / direct run, exit 0)
- `cargo test -p dblocal-core -p dblocal-gui -p dblocal-cli -p xtask`: **PASS** (see xtask ci results; `cargo test -p dblocal-core`: 52 passed)
- `cargo run -p dblocal-cli -- --help`: **PASS** (exit 0)
  - Printed usage with commands: `list status start stop uri create delete logs help`
- `cargo run -p dblocal-cli` (no args): **PASS** (exit 0)
  - GUI stub `dblocal_gui::run()` returns `Ok(())` per Phase 0 behavior/evidence requirement
- `cargo run -p dblocal-cli -- list`: **PASS contract behavior** (stub subcommand, exit 1)
  - `[DBL-9001] Fitur ini belum tersedia di versi ini.` / `RC:1`
- `cargo run -p dblocal-cli -- bogus` (unknown subcommand): **PASS contract behavior** (exit 2)
  - `error: unrecognized subcommand 'bogus'` / `RC:2`
- `cargo run -p xtask -- catalog`: **PASS stub contract** (exit 1, prints `not implemented`)
- `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps -p dblocal-core`: **PASS** (exit 0, zero warnings)
  - `Generated /home/rezkycodes/.cache/dblocal-target/doc/dblocal_core/index.html`
- `cargo check -p dblocal-core --features mock`: **PASS** (empty `service/mock.rs` behind feature)
- `cargo deny check licenses bans`: **FAIL (environment/ownership)** — `deny.toml` is owned by W0-D and absent in this workspace; default policy rejects many transitive licenses (e.g. MIT/Apache-2.0 crates flagged as not explicitly allowed under default config). `bans ok`. Cargo.lock changed as required by workspace creation; license policy enforcement is W0-D’s responsibility. Not fixed here because I must not create `deny.toml`.
- `cargo llvm-cov -p dblocal-core --summary-only`: **PASS**

## 4. Coverage (files owned in dblocal-core)

Overall: **95.97% lines**, **96.34% regions**, **94.91% functions** (52 tests).

| File | Lines | Missed | Cover |
|---|---:|---:|---:|
| engine/mod.rs | 86 | 0 | 100.00% |
| error.rs | 129 | 1 | 99.22% |
| model/artifacts.rs | 455 | 7 | 98.46% |
| model/entity.rs | 359 | 12 | 96.66% |
| model/mod.rs | 29 | 0 | 100.00% |
| model/scalar.rs | 211 | 14 | 93.36% |
| model/state.rs | 164 | 4 | 97.56% |
| paths.rs | 193 | 20 | 89.64% |
| platform.rs | 83 | 6 | 92.77% |
| secret.rs | 45 | 3 | 93.33% |
| service/mod.rs | 57 | 6 | 89.47% |
| **TOTAL** | **1811** | **73** | **95.97%** |

## 5. Deviations from task/PRD/contract and why
- **`spawn_command` signature:** implemented as `fn spawn_command(&self, ctx: &RunContext) -> Command` (infallible) to match PRD §9’s `EngineAdapter` sketch and the stated contract in `CONTRACT.md`; stub adapters cannot return a real `NotImplemented` `Command`, so they return a placeholder `dblocal-stub` command and surface `NotImplemented` via fallible operations (`initialize/health_check/shutdown`). This keeps the trait aligned with the PRD while preserving stub behavior.
- **GUI stub returns `Ok(())`:** `dblocal_gui::run()` logs and returns success so `cargo run -p dblocal-cli` exits 0 as required by the task evidence; real GPUI app is W1-G’s responsibility.
- **CLI prints `err.diagnostic()` (`[DBL-NNNN] ...`):** preserves stable error codes in user-visible output while keeping `DblError::user_message()`/`Display` semantics consistent; exit codes still follow `F-CLI`.
- **Workspace deps added in `[workspace.dependencies]` and declared in `crates/core`:** all requested crates declared; `keyring` intentionally omitted with `# W1-E adds keyring` comment. Added `gpui = { package = "gpui-pre", version = "=0.3.12" }`, `gpui-component = "=0.10.0"`, `tray-icon = "0.26"` to `[workspace.dependencies]` (not consumed yet) to satisfy AGENTS pin table and ease W1-G without Cargo.toml conflicts; `cargo metadata`/`cargo check` resolve cleanly.
- **`cargo deny check licenses bans` not green:** no `deny.toml` (owned by W0-D), so default config rejects licenses; I did not create it.
- **No `git push/merge/rebase`:** respected worker rules.

## 6. Open issues / reviewer notes
- **License gate ownership:** W0-D must add/configure `deny.toml` (and allow GPL-3.0-only compatible set) so `cargo deny` passes on this lockfile.
- **Unused dependency warnings:** many deps are predeclared in core (per task) but not yet referenced by implemented modules; they compile fine now but may surface `unused_crate_dependencies` if linting is tightened later. Wave 1 workers will consume them.
- **`paths.rs`/`service/mod.rs` coverage < 90% line-wise in some helpers:** overall core is 95.97% and every owned file is ≥89.64%; additional edge-case tests can be added in follow-up if reviewer wants stricter per-file targets.
- **`reqwest` 0.13 feature name:** used `features = ["rustls", "stream", "json"]` (reqwest 0.13 renamed `rustls-tls` → `rustls`) to satisfy the pinned version; tls remains rustls-only with `default-features = false`.
- **CLI clap parsing/printing:** `run()` currently prints clap errors/help directly for correct UX/exit codes; tests exercise exit-code paths (including `--help` and unknown subcommand) and may emit clap stderr/stdout during test runs (harmless but visible).
- **`docs/architecture/W0-A-REPORT.md` added:** written under owned path to capture full evidence; not part of original deliverable list but useful for review (can be removed/renamed if undesired).

## Required tests implemented
- State machine: table test covers every valid `F-INS` transition and asserts `InvalidTransition` for every other (status,event) pair.
- `InstanceName` and `Port` boundary cases; `EngineKey` string round trip/families/ports.
- `DblPaths` layout under `with_root`, unsafe segment rejection, `resolve` honors `DBLOCAL_HOME` semantics without mutating process env.
- `SecretString` never leaks via `Debug`/`Display`/nested `Debug`.
- Error codes unique and match `^DBL-\d{4}$`; `user_message()` non-empty for every variant.
- CLI: parses PRD §8 commands, `--help`/no-args succeed (exit 0), unknown subcommand rejected (exit 2), stub subcommands exit 1, exit-code mapping covered.