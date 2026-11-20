# W0-A Review Round 1 — Fixes (M1–M4)

## Round-1 summary
- Fixed all blocking review items M1–M4 on `rezkycodes/w0-skeleton` in 5 commits (top of `5ce8019`): state-machine recovery events, strict artifact validation, auth-aware engine contexts/start diagnosis, and the extended `DblService`/read-model contract with live-log opt-in.
- Gates: fmt, clippy -D warnings, full `cargo xtask ci` (8 CLI + 70 core tests), workspace build, strict rustdoc, mock feature, and CLI exits 0/0/2/1 all pass; core coverage 95.64% lines with every owned file >= 89.64%.
- `cargo deny check licenses bans` still fails because `deny.toml` is absent/owned by W0-D (default config rejects transitive licenses); `bans ok`. No merge performed.

## Branch / diff (this round only)
- Base (round start): `5ce8019 docs(core): add W0-A evidence report`
- Head: `3c0a6ef docs(contract): document review-round-1 contract changes`
- Round commits:
  - `008db3e feat(state): add ReconciledAbsent and Interrupted recovery events`
  - `f6e49d6 fix(catalog): require https urls and non-zero artifact size`
  - `7098030 feat(engine): make contexts auth-aware and add start-failure diagnosis`
  - `72539cc feat(service): extend read models and service contract for PRD flows`
  - `3c0a6ef docs(contract): document review-round-1 contract changes`

```
git diff --stat 5ce8019..HEAD
 Cargo.lock                         |    1 +
 Cargo.toml                         |    1 +
 crates/core/Cargo.toml             |    1 +
 crates/core/src/engine/mod.rs      |  127 +++++-
 crates/core/src/model/artifact.rs  |  338 ++++++++++++++++
 crates/core/src/model/artifacts.rs |  763 -------------------------------------
 crates/core/src/model/catalog.rs   |  296 ++++++++++++++
 crates/core/src/model/entity.rs    |   36 +-
 crates/core/src/model/failure.rs   |  103 +++++
 crates/core/src/model/mod.rs       |   13 +-
 crates/core/src/model/runtime.rs   |  240 ++++++++++++++++++
 crates/core/src/model/scalar.rs    |    9 +
 crates/core/src/model/state.rs     |   82 +++-
 crates/core/src/secret.rs          |   32 +-
 crates/core/src/service/error.rs   |  106 ++++++
 crates/core/src/service/mod.rs     |  183 +++++++---
 crates/core/src/service/view.rs    |  180 +++++++++
 docs/architecture/CONTRACT.md      |   91 ++++-
 18 files changed, 1733 insertions(+), 869 deletions(-)
```

## M1–M4 changes and new tests

### M1 (state machine, PRD F-INS-04)
- Added `InstanceEvent::ReconciledAbsent` (`Starting|Running|Stopping -> Stopped`, normal post-reboot/PID-reuse recovery per D-09/F-INS-04 AC2) and `InstanceEvent::Interrupted` (`Provisioning|Initializing -> Failed`, crash mid-operation). `ProcessExited` remains `Running -> Failed` for observed crashes.
- Table/exhaustive tests extended (`ALL_EVENTS` now 16, `diagram_edges` now 22) so every valid/invalid pair remains covered.
- New/updated tests: `model::state::tests::every_diagram_edge_is_accepted`, `model::state::tests::every_other_pair_is_rejected`, `model::state::tests::killing_a_process_observed_alive_still_fails`, `model::state::tests::recovery_events_have_stable_hints`.

### M2 (artifact validation)
- `Artifact::new` rejects any non-HTTPS URL (`InvalidInput { field: "url" }`, including http/file/ftp/schemeless/empty/uppercase scheme/whitespace) and `size_bytes == 0` (`field: "size_bytes"`), in addition to SHA-256.
- New tests: `model::artifact::tests::artifact_rejects_non_https_schemes`, `model::artifact::tests::artifact_rejects_zero_size`, `model::artifact::tests::artifact_accepts_a_well_formed_entry`.

### M3 (EngineAdapter / diagnosis)
- `InitContext { ..., auth, password: Option<SecretString> }` (`Some` iff `AuthMode::Password`), `RunContext { ..., auth, credential: Option<Credential> }`; `AuthMode` controls bootstrap/auth. `AdminOps` methods read credential from context (documented).
- Added `StartFailure { PortInUse, DataDirVersionMismatch, PermissionDenied, BinaryMissing, Unknown }` (`#[non_exhaustive]`, stable `as_str()`) and `EngineAdapter::diagnose_start_failure(&self, output, exit_code) -> StartFailure` defaulting to `Unknown`.
- Rustdoc: `shutdown` blocks bounded by `STOP_GRACE_SECS`, `health_check` is protocol-level and treats auth challenges (Valkey NOAUTH / PostgreSQL auth request) as `Ready`.
- New/updated tests: `model::failure::tests::every_variant_round_trips_and_is_unique`, `model::failure::tests::only_actionable_variants_offer_a_fix`, `model::failure::tests::default_is_unknown`, `engine::tests::auth_is_consistent_in_contexts`, `engine::tests::stubs_answer_not_implemented` (asserts default diagnosis), `engine::tests::stub_uri_is_redacted`.

### M4 (DblService / read models)
- `InstanceSpec.port` is `Option<Port>` (`None` = allocate). `InstanceView` adds `auth`, `autostart`, `credential_backend`, `failure` (`FailureInfo { code, message, diagnosis, log_tail }`).
- `CatalogView` is a platform-filtered read model `{ status, banner, engines: Vec<EngineEntry> }` with `EngineEntry { engine, available, unavailable_reason, versions: Vec<VersionEntry> }`, `VersionEntry { version, eol, installed, size_bytes }`.
- New methods: `refresh_catalog(force)`, `validate_spec`, `suggest_port`, `change_port`, `data_size`, `set_autostart`, `watch_logs(id, enabled)`, `list_runtimes`, `remove_runtime`, `list_archives`, `purge_archive` (existing methods preserved).
- `ServiceEvent::OperationFinished { id, result: Result<(), UserFacingError> }` with `UserFacingError { code, message, diagnosis }` and `From<&DblError>`; `InstanceChanged` carries full `InstanceView`; added `LogLines { id, lines }` for live logs. CONTRACT documents opt-in `watch_logs` choice.
- New tests: `service::error::tests::conversion_keeps_code_and_message`, `service::error::tests::conversion_never_leaks_a_payload`, `service::error::tests::diagnosis_is_attached_explicitly`, `service::error::tests::direct_construction_matches_error_shape`, `service::view::tests::instance_view_exposes_auth_and_badges`, `service::view::tests::catalog_view_carries_availability`, `service::tests::operation_finished_preserves_the_error_code`, `service::tests::instance_changed_carries_the_full_view`, `service::tests::log_lines_event_is_shape_only` (plus updated `service::tests::view_reflects_the_instance`).

### Nits
- `SecretString` now zeroes its buffer on drop (`zeroize = { version = "1", features = ["alloc"] }`, `Zeroizing<String>` + `Zeroize` impl) and keeps Debug/Display redacted; doc fixed. New test: `secret::tests::zeroize_clears_the_backing_buffer`.
- File sizes kept < 800 lines by splitting `model/artifacts.rs` into `model/artifact.rs`, `model/catalog.rs`, `model/runtime.rs` and splitting service into `service/mod.rs`, `service/view.rs`, `service/error.rs`.

## Quality gates

- `cargo fmt --all -- --check`: **PASS**
- `cargo clippy --workspace --all-targets -- -D warnings`: **PASS** (exit 0)
- `cargo build --workspace`: **PASS** (`Finished \`dev\` profile [unoptimized + debuginfo] target(s) in 1.33s`)
- `cargo xtask ci`: **PASS** (`fmt check` + `clippy` + `tests` all green)
  - `test result: ok. 8 passed; 0 failed` (dblocal-cli)
  - `test result: ok. 70 passed; 0 failed` (dblocal-core lib)
  - `test result: ok. 0 passed; 0 failed` (dblocal-gui, xtask, doc-tests)
  - `xtask ci: all gates passed`
- `cargo run -p dblocal-cli` (no args): **RC 0**
- `cargo run -p dblocal-cli -- --help`: **RC 0**
- `cargo run -p dblocal-cli -- bogus` (unknown): **RC 2**
- `cargo run -p dblocal-cli -- list` (stub subcommand): **RC 1**
- `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps -p dblocal-core`: **PASS** (exit 0, zero warnings)
- `cargo check -p dblocal-core --features mock`: **PASS**
- `cargo deny check licenses bans`: **FAIL (pre-existing, W0-D ownership)** — `deny.toml` absent; default config rejects many MIT/Apache-2.0 transitives while `bans ok`. Not fixed because W0-D owns `deny.toml`.

## Coverage (`cargo llvm-cov -p dblocal-core --summary-only`)

```
Filename                      Regions    Missed Regions     Cover   Functions  Missed Functions  Executed       Lines      Missed Lines     Cover    Branches   Missed Branches     Cover
-----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------
engine/mod.rs                     210                 3    98.57%          18                 0   100.00%         142                 0   100.00%           0                 0         -
error.rs                          155                 2    98.71%           8                 0   100.00%         129                 1    99.22%           0                 0         -
model/artifact.rs                 244                 5    97.95%          26                 0   100.00%         207                 6    97.10%           0                 0         -
model/catalog.rs                  244                 3    98.77%          26                 0   100.00%         175                 2    98.86%           0                 0         -
model/entity.rs                   484                26    94.63%          62                 8    87.10%         362                26    92.82%           0                 0         -
model/failure.rs                   68                 0   100.00%           6                 0   100.00%          43                 0   100.00%           0                 0         -
model/mod.rs                       49                 0   100.00%           6                 0   100.00%          29                 0   100.00%           0                 0         -
model/runtime.rs                  157                 2    98.73%          19                 0   100.00%         129                 1    99.22%           0                 0         -
model/scalar.rs                   349                23    93.41%          38                 5    86.84%         215                18    91.63%           0                 0         -
model/state.rs                    240                 9    96.25%          17                 1    94.12%         210                 4    98.10%           0                 0         -
paths.rs                          323                24    92.57%          31                 3    90.32%         193                20    89.64%           0                 0         -
platform.rs                       120                 9    92.50%          16                 1    93.75%          83                 6    92.77%           0                 0         -
secret.rs                          82                 3    96.34%          13                 1    92.31%          54                 3    94.44%           0                 0         -
service/error.rs                   76                 0   100.00%           9                 0   100.00%          57                 0   100.00%           0                 0         -
service/mod.rs                    113                11    90.27%          11                 0   100.00%          83                 8    90.36%           0                 0         -
service/view.rs                    91                 0   100.00%           6                 0   100.00%          70                 0   100.00%           0                 0         -
-----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------
TOTAL                            3005               120    96.01%         312                19    93.91%        2181                95    95.64%           0                 0         -
```

## Round 2 (M5: trim `zip` features)

- **Change:** `zip = "9"` -> `zip = { version = "9", default-features = false, features = ["deflate"] }` (root `Cargo.toml`). We only read ordinary deflate zip archives and extraction is on untrusted input, so extra codecs were dropped.
- **Dependency tree size:** `cargo tree --workspace | wc -l` **612 -> 577** (delta **-35**).
- **Removed crates (19, from `Cargo.lock`):** `aes`, `bzip2`, `cipher`, `cpubits`, `deflate64`, `deranged`, `inout`, `libbz2-rs-sys`, `lzma-rust2`, `num-conv`, `pbkdf2`, `powerfmt`, `ppmd-rust`, `sha1`, `time`, `time-core`, `zstd`, `zstd-safe`, `zstd-sys`. None added.
- **Verification:** `cargo tree --workspace -i libbz2-rs-sys` reports nothing (exit 1, no match); `grep -cE 'name = "(libbz2-rs-sys|bzip2)"' Cargo.lock` prints `0`.
- **Nit:** `SecretString` (crates/core/src/secret.rs) gained a doc note that `==`/`PartialEq` must not be used to compare secrets from untrusted input (derived comparison is not constant-time); existing behavior/tests unchanged.
- **Not done:** `.commandcode/` left uncommitted (ignored on `main`). `deny.toml` is not present in this worktree, so `cargo deny check licenses bans` was not re-run here; the targeted licence source (`libbz2-rs-sys`/`bzip2`) is removed from the lock.
