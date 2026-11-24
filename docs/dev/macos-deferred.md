# macOS deferred for the first release

**macOS is deferred for the first release: Linux and Windows ship first, and macOS is postponed.**
To revive it, set the repository variable `ENABLE_MACOS=true` (Settings > Secrets and variables >
Actions > Variables) and follow "Reviving macOS" below.

## Current state (what "deferred" means in the workflows)

- `.github/workflows/ci.yml`: the `test` job keeps `macos-15` and `macos-15-intel` in the matrix
  but sets `continue-on-error: ${{ startsWith(matrix.os, 'macos') }}`, so a macOS failure does not
  turn the run red (GitHub docs, `jobs.<job_id>.continue-on-error`: "Prevents a workflow run from
  failing when a job fails"). The legs stay visible in the logs.
- `.github/workflows/release.yml`: `build-macos-arch` and `build-macos-universal` carry
  `if: ${{ vars.ENABLE_MACOS == 'true' }}`, so they are skipped by default. The `release` job no
  longer depends on the macOS job succeeding — it requires the Linux and Windows jobs to succeed
  and ignores the macOS result, so Linux/Windows artifacts are always published.
- `rust-toolchain.toml` and every crate pin are unchanged: Linux stays on **1.95.0**.

Enable macOS again by setting the repository variable `ENABLE_MACOS=true`
(Settings > Secrets and variables > Actions > Variables) and following "Reviving macOS" below.

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

1. Set repository variable `ENABLE_MACOS=true`.
2. In `.github/workflows/ci.yml`, replace `continue-on-error: ${{ startsWith(matrix.os, 'macos') }}`
   with `false` (or delete the line).
3. In `.github/workflows/release.yml`, drop `if: ${{ vars.ENABLE_MACOS == 'true' }}` from
   `build-macos-arch` and `build-macos-universal`, and restore `build-macos-universal` to the
   `release` job's required `needs` results.
4. Add the macOS toolchain step back to `ci.yml` (macOS leg only):
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

`continue-on-error: ${{ runner.os == 'macOS' }}` does **not** work: `runner` is not an allowed
context there. GitHub allows only `github`, `needs`, `strategy`, `vars`, `matrix`, `inputs` for
`jobs.<job_id>.continue-on-error`, and `actionlint` rejects the `runner.os` form
(`context "runner" is not allowed here`). Use `startsWith(matrix.os, 'macos')`, as `ci.yml` does.
A job-level `if:` cannot replace it either: `jobs.<job_id>.if` also cannot see `matrix`.
