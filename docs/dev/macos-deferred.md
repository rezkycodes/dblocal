# macOS deferred for the first release

**macOS is deferred for the first release: Linux and Windows ship first, and macOS is postponed.**
To revive it, either set the repository variable `ENABLE_MACOS=true` (Settings > Secrets and
variables > Actions > Variables) or start the CI workflow by hand with **Run workflow**
(`workflow_dispatch`). See "Reviving macOS" below — turning the legs on is not enough on its own;
a toolchain step must be added first.

## Current state (what "deferred" means in the workflows)

- `.github/workflows/ci.yml`: the `test` job computes its `matrix.os` from `vars.ENABLE_MACOS` and
  `github.event_name`. On a normal push or pull request the matrix is exactly
  `ubuntu-24.04`, `ubuntu-24.04-arm`, `windows-latest`; the `macos-15` and `macos-15-intel` legs are
  added only when `ENABLE_MACOS == 'true'` or the run was started with `workflow_dispatch`. Gating
  the matrix (rather than `continue-on-error`) means a macOS failure on an opt-in run is a real red
  failure, which is what you want when you are deliberately testing macOS. There is no
  `continue-on-error` on the job any more.
- The `concurrency` group includes `github.event_name`, so a manual run is never cancelled by, and
  never cancels, a push/PR run on the same ref.
- `.github/workflows/release.yml`: `build-macos-arch` and `build-macos-universal` carry
  `if: ${{ vars.ENABLE_MACOS == 'true' }}`, so they are skipped by default. The `release` job no
  longer depends on the macOS job succeeding — it requires the Linux and Windows jobs to succeed
  and ignores the macOS result, so Linux/Windows artifacts are always published.
- `rust-toolchain.toml` and every crate pin are unchanged: Linux stays on **1.95.0**.

## Why the legs are off by default

CI costs. GitHub Actions minutes are billed by the runner's cost multiplier, and macOS runs at
**10x** the Linux rate while the first release does not ship a macOS artifact at all. Keeping two
macOS legs in every push and pull request spent that budget on builds that could not go green (see
the failure below). They are now opt-in, and on a private repository that is the difference between
a CI bill you can predict and one you cannot.


## The failure that started this

Run `44973248120` (commit `0e943b3`), `macos-15`, verbatim:

```
error[E0658]: use of unstable library feature `result_option_map_or_default`
  --> .../gpui-pre-macos-0.3.12/src/window.rs:896:14
  --> .../gpui-pre-macos-0.3.12/src/window.rs:957:14
  --> .../gpui-pre-macos-0.3.12/src/window.rs:1068:18
error: could not compile `gpui-pre-macos` (lib) due to 3 previous errors
```

## Root cause, with evidence

1. `gpui-pre-macos 0.3.12` calls `Option::map_or_default` / `Result::map_or_default`. Those
   methods were `#[unstable]` (tracking issue rust-lang/rust#138099) until the stabilization PR
   **rust-lang/rust#156222** (merged 2026-06-07) shipped in **Rust 1.98.0** (Rust 1.98.0 release
   notes list `Option::map_or_default` and `Result::map_or_default`).
2. The job asks `dtolnay/rust-toolchain` for `stable` (which resolved to rustc 1.100.0), but
   `cargo` runs inside the checkout where `rust-toolchain.toml` pins `channel = "1.95.0"`. That
   file is a rustup **directory override** and wins; the job log says so:
   `info: note that the toolchain '1.95.0-aarch64-apple-darwin' is currently in use (overridden by
   '/Users/runner/work/db-local/db-local/rust-toolchain.toml')`. So the compile that failed ran on
   **1.95.0**, not on the `stable` the action installed.
3. Local proof: `rustc 1.95.0` rejects `x.map_or_default()` with E0658; `rustc 1.98.0` accepts it.
4. Upgrading the crate does **not** help: `gpui-pre-macos` 0.3.13 and 0.3.14 still call
   `map_or_default` (checked in the published sources). The lever is the toolchain, not the crate
   version. Note `gpui-component 0.10.0` hard-requires `gpui-pre =0.3.12`, and `gpui-pre 0.3.13`
   does not resolve against it.
5. Only the macOS backend uses the API: `gpui-pre-macos` is the only macOS platform crate in the
   resolved graph, and `gpui-pre-apple` does not call `map_or_default`. Linux builds
   `gpui-pre-linux`, Windows `gpui-pre-windows`, so neither is affected.

## Second and third blockers found while investigating

The macOS legs fail on our own code too, and one of them also fails **Windows**:

- `crates/core/src/process/probe.rs`: the test module imports `std::time::Duration` but only the
  `#[cfg(target_os = "linux")]` test uses it, so `clippy -D warnings` reports
  `error: unused import: std::time::Duration` on any non-Linux target.
- `crates/core/src/engine/mariadb/identity.rs`: six functions used only by the Linux identity
  check (`same_executable`, `strip_deleted`, `parse_cmdline`, `cmdline_targets_instance`,
  `path_arg_matches`, `is_zombie_stat`) are reachable only from `#[cfg(target_os = "linux")]`
  code, so a non-Linux **lib** build reports each as `never used` (dead code) under
  `-D warnings`. Confirmed on run `45347770357` (commit `e386404`): both macOS legs and the
  `windows-latest` leg fail with these six dead-code errors.

So deferring macOS alone does not make CI green: **Windows is red for the same six errors**.
Those belong to the Windows CI owner, not this change.

## Fix options and trade-offs

| Option | Effect | Risk to Linux/Windows | Notes |
|---|---|---|---|
| **A. Bump only the macOS job to Rust 1.98.0** (`rustup toolchain install 1.98.0` + `RUSTUP_TOOLCHAIN=1.98.0`, which beats a directory override) | Compiles the gpui stack; E0658 disappears | None (Linux/Windows untouched) | Verified locally that both 1.95.0 and 1.98.0 pass fmt, clippy and all 2209 tests. Still leaves the dead-code and unused-import errors to fix; a newer clippy also adds `chunks_exact_to_as_chunks` in `xtask`. |
| B. Bump the whole repo to 1.98.0 (`rust-toolchain.toml`) | One toolchain everywhere | Touches the green Linux/Windows legs; violates the "keep 1.95.0" rule | Rejected for now. |
| C. Patch/vendor `gpui-pre-macos` to avoid `map_or_default` | Fixes the dependency | New `[patch]`/vendor maintenance burden | `E0658` needs `#![feature(...)]`, which cannot be added to a registry dependency without vendoring; `RUSTC_BOOTSTRAP` alone does not enable the method. |
| D. Downgrade to a `gpui-pre` without the call (0.3.9) | No `map_or_default` | Regresses the whole GUI stack | `gpui-component 0.10.0` requires `=0.3.12`. Rejected. |

Chosen for now: **defer** (this change). When macOS is revived, start from option **A**.

## Reviving macOS (recipe)

1. Either set repository variable `ENABLE_MACOS=true` (every push and PR then runs the macOS legs),
   or leave it unset and start each macOS run by hand: **Actions > CI > Run workflow**. Prefer the
   manual route while macOS is still red — it costs nothing on ordinary pushes.
2. Expect the first opt-in run to be **red**, not just slow: `ci.yml` does not yet install Rust
   1.98.0 for macOS, so the leg still hits the `map_or_default` E0658 error. That is deliberate —
   the switch above only costs minutes when you ask for them. Step 3 fixes the toolchain.
3. Add the macOS toolchain step to `ci.yml` before the CI task (macOS leg only):
   ```yaml
   - name: Use Rust 1.98.0 for the macOS gpui stack
     if: runner.os == 'macOS'
     shell: bash
     run: |
       rustup toolchain install 1.98.0 --profile minimal \
         --component clippy,rustfmt --no-self-update
       echo "RUSTUP_TOOLCHAIN=1.98.0" >> "$GITHUB_ENV"
   ```
   (Must be *after* checkout: `rust-toolchain.toml` only exists after checkout, and
   `RUSTUP_TOOLCHAIN` overrides it.)
4. In `.github/workflows/release.yml`, drop `if: ${{ vars.ENABLE_MACOS == 'true' }}` from
   `build-macos-arch` and `build-macos-universal`, and restore `build-macos-universal` to the
   `release` job's required `needs` results (only when you want macOS artifacts gated as release
   blockers; leaving the `if:` in place keeps release.yml driven by the same variable).
5. Fix the our-code blockers before the legs can go green (they also block Windows). As of
   `ci/linux-windows` (`3ec1016`) the first two are already fixed there; re-check on whatever
   branch you revive from:
   - gate `use std::time::Duration` in `crates/core/src/process/probe.rs` tests to
     `#[cfg(target_os = "linux")]` (**already fixed on `ci/linux-windows`**);
   - give the six `mariadb::identity` helpers a `#[cfg(any(target_os = "linux", test))]` gate
     (their callers are all Linux-only or test-only) (**already fixed on `ci/linux-windows`**);
   - after moving to 1.98.0, `crates/xtask/src/catalog/sources/mysql.rs` must use
     `message.as_chunks::<64>().0` instead of `chunks_exact(64)` (new 1.98 lint; the earlier
     `as_chunks` change was reverted in `4e95c01` and must be reapplied).
6. Re-enable Apple signing/notarization secrets when a Developer ID is available.

## Not proven here

There is no macOS machine. Everything above is from CI logs, published crate sources, rustc
behaviour on this host, and official docs. Only a real macOS CI run can confirm the legs go green.

## Note on the expression form

`jobs.<job_id>.continue-on-error` cannot use the `runner` context — GitHub allows only `github`,
`needs`, `strategy`, `vars`, `matrix`, `inputs` there, and `actionlint` rejects the `runner.os` form
(`context "runner" is not allowed here`).

`jobs.<job_id>.if` also cannot see `matrix`, so it cannot turn individual matrix legs on or off. That
is why `ci.yml` gates the **matrix itself**: `matrix.os` is computed with `fromJSON(...)` from
`vars.ENABLE_MACOS` and `github.event_name`, so the macOS labels are simply not in the list unless
they are switched on. One job, one step list, no `continue-on-error`.
