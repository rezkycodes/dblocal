# macOS CI toolchain note (gpui-pre-macos and Rust 1.95)

Both macOS jobs in CI run on Rust **1.98.0** while every other job, and this machine, use the
repo pin **1.95.0**. This document records why, so nobody "simplifies" it back.

## What failed

Run `44973248120` (commit `0e943b3`, the last pushed commit) failed on `macos-15` and
`macos-15-intel` with three errors in a dependency, not in our code:

```
error[E0658]: use of unstable library feature `result_option_map_or_default`
  --> .../gpui-pre-macos-0.3.12/src/window.rs:896:14
  --> .../gpui-pre-macos-0.3.12/src/window.rs:957:14
  --> .../gpui-pre-macos-0.3.12/src/window.rs:1068:18
```

`gpui-pre-macos 0.3.12` calls `Option::map_or_default` / `Result::map_or_default`. Those methods
were `#[unstable]` (rust-lang/rust#138099) until the stabilization PR rust-lang/rust#156222
merged 2026-06-07 and shipped in **Rust 1.98.0** (see that release's notes). On 1.95.0 they do
not exist on stable; on 1.98.0 they do.

## Why the job used 1.95.0 although CI asked for `stable`

`dtolnay/rust-toolchain` is asked for `stable`, which resolved to rustc 1.100.0, and the job log
prints `rustc 1.100.0`. But `cargo` then runs inside the checkout, where `rust-toolchain.toml`
pins `channel = "1.95.0"`. That file is a rustup **directory override**, and the job log says so
explicitly:

```
info: note that the toolchain '1.95.0-aarch64-apple-darwin' is currently in use
      (overridden by '/Users/runner/work/db-local/db-local/rust-toolchain.toml')
```

So the compile that produced E0658 ran on 1.95.0. Installing a newer `stable` in CI cannot fix
this; the override wins.

## The fix

A macOS-only step installs 1.98.0 and sets `RUSTUP_TOOLCHAIN=1.98.0`. The environment variable
beats a `rust-toolchain.toml` directory override (verified locally: with
`RUSTUP_TOOLCHAIN=1.98.0`, `rustc --version` reports 1.98.0 inside a 1.95.0-pinned directory).
Linux and Windows keep 1.95.0 on purpose.

## Why only macOS needs the bump

`map_or_default` appears in exactly one crate of our resolved graph: `gpui-pre-macos`. On a Linux
build the macOS backend is not compiled at all (`gpui-pre-linux` is), and on Windows
`gpui-pre-windows` is; `gpui-pre-apple` does not use the API. Upgrading `gpui-pre` does not help
either: 0.3.13 and 0.3.14 still call `map_or_default`, so the toolchain is the lever, not the crate
version.

## Second, independent blocker

`crates/core/src/process/probe.rs` imported `std::time::Duration` in its test module, but only the
`#[cfg(target_os = "linux")]` test uses it. On any non-Linux target `clippy -D warnings` failed
with `error: unused import: std::time::Duration`. The import is now gated to Linux. This is why the
Intel job, which reached our code before the failing dependency (see its log), showed only this
error.

`crates/xtask/src/catalog/sources/mysql.rs` used `chunks_exact(64)`, which clippy 1.98.0 flags
(`chunks_exact_to_as_chunks`). It now uses `as_chunks::<64>()`, stable on both 1.95.0 and 1.98.0;
the RFC-1321 MD5 vector test still passes on both.

## What is proven, and what is not

Proven here, from command output: the E0658 text and line numbers (CI log); 1.95.0 rejects
`map_or_default` and 1.98.0 accepts it (local compiles); the toolchain override and precedence
(`rustup show active-toolchain`, CI log); the unused-import lint and its fix (a minimal
`--target x86_64-pc-windows-gnu` repro, before and after); that `xtask`, `dblocal-core`,
`dblocal-gui` and `dblocal-cli` pass fmt, clippy `-D warnings` and all 2209 tests under **both**
1.95.0 and 1.98.0 on Linux.

Not proven here: that the macOS jobs now pass green. There is no macOS machine; only a CI run on
these commits can confirm it.
