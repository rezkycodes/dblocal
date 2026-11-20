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

## Ownership (Wave 1)

| Task | Branch | Owns |
|---|---|---|
| W1-A | `w1-store` | `core/src/store*`, `docs/architecture/store.md` |
| W1-B | `w1-catalog` | `core/src/catalog*`, `crates/xtask/**`, `catalog/**` (additive fields in `model/catalog.rs`, `model/artifact.rs`) |
| W1-C | `w1-download` | `core/src/download*` (fetch, verify, extract) |
| W1-D | `w1-process` | `core/src/process*`, `core/src/ports*` |
| W1-E | `w1-secrets` | `core/src/secrets*`, plus the `keyring` dependency lines |
| W1-F | `w1-engines` | `core/src/engine/{postgres,valkey,uri,glibc}*` (URI building moved here from W1-E) |
| W1-G | `w1-gui-shell` | `crates/gui/**`, `core/src/service/mock.rs`, `core/src/strings.rs` (only W1-G edits strings in Wave 1) |

Launch order (machine load): F, B, C, D first; A and E when slots free; G after W0-C reports.
`strings.rs` is shared and append-only, so only W1-G edits it in this wave; core workers return typed errors.

## Status

| Task | Status | Notes |
|---|---|---|
| W0-A | round 2 in progress | R1 verified (gates, 95.64% coverage, M1-M4 correct). Open: M5 `zip` default features pull `bzip2` (only crate `cargo deny` rejects) |
| W0-B | **merged** (952afaa) | Findings verified independently (MySQL 832 MB, no SHA-256; MariaDB Linux/Windows x64 only; Valkey Linux only). Reviewer found MySQL `-minimal` 79.8 MB. Recorded as D-24 |
| W0-C | running | Redirected to Zed-style UI (D-23); waiting for go/no-go and widget coverage table |
| W0-D | **merged** | Round 2 re-verified: 52/52 tests (12 fail on the old installer, so they are not vacuous), hostile `DBLOCAL_RELEASE_BASE_URL` values rejected with 0 curl calls, shellcheck clean. Not run on GitHub: `release.yml`, `valkey-build.yml` |
| W1-A..F | specs written | Start after W0-A merges |
| W1-G | spec waits for W0-C | |
