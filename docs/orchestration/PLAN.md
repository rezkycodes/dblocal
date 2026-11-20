# Build plan: orchestration waves

Coordinator and reviewer: Claude. Workers: Command Code (`--agent command-code`) via Orca, one git
worktree and branch per worker. Rule: **no worker edits a path another worker owns.** The skeleton
(W0-A) pre-declares every module and dependency so later waves never conflict on `lib.rs` or `Cargo.toml`.

## Waves

| Wave | Goal | Tasks (parallel inside a wave) | Gate to enter next wave |
|---|---|---|---|
| 0 | Contract + de-risk | W0-A skeleton · W0-B engine spikes (PostgreSQL, Valkey on Linux) · W0-C GPUI + tray spike · W0-D CI, packaging, legal | W0-A merged and reviewed |
| 1 | Core modules + GUI shell | W1-A store · W1-B catalog + `xtask catalog` · W1-C download + extract · W1-D process + ports · W1-E secrets + URIs · W1-F engine adapters (PostgreSQL, Valkey) · W1-G GUI shell + mock service | all Wave 1 merged, `cargo xtask ci` green |
| 2 | Integration (Phase 1 done) | W2-A instance service (`DblService` real impl) · W2-B GUI wiring, logs, tray, quit dialogs · W2-C e2e on real PostgreSQL + Valkey | Phase 1 definition of done in PRD §11 |
| 3 | Phase 2 | MySQL/MariaDB adapters · database and user admin · open-in-client · CLI · deb/rpm/msi · self-update | Phase 2 definition of done |
| 4 | Phase 3 | config editor · autostart · accessibility and polish | Phase 3 definition of done |

## UI direction

Decision D-23: the GUI is Zed-style. W1-G and every GUI-facing task starts from `docs/design/UI-DIRECTION.md`. W0-C's findings decide which
`gpui-component` widgets cover the layout and which parts we build on raw GPUI.

## Ownership (Wave 0)

| Task | Branch | Owns |
|---|---|---|
| W0-A | `w0-skeleton` | `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`, `rustfmt.toml`, `clippy.toml`, `.cargo/`, `crates/**`, `docs/architecture/**` |
| W0-B | `w0-engine-spikes` | `spikes/engines/**`, `docs/spikes/engines/**` |
| W0-C | `w0-gpui-spike` | `spikes/gpui/**`, `docs/spikes/gpui/**` |
| W0-D | `w0-ci-packaging` | `.github/**`, `packaging/**`, `scripts/**`, `deny.toml`, `COPYING`, `README.md`, `docs/release/**`, `docs/install/**` |

## Review protocol

For each finished worker: read the report, check `git diff --stat`, re-run the gates myself on the
branch, read the code against the PRD acceptance criteria and AGENTS.md invariants, then merge or send
fixes back with `orchestration send`. Findings go into the status table below.

## Status

| Task | Status | Notes |
|---|---|---|
| W0-A | not started | |
| W0-B | not started | |
| W0-C | not started | |
| W0-D | not started | |
