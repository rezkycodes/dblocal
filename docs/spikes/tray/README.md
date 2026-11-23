# W2-G — Windows/macOS tray backend: compile evidence and evaluation

Branch `rezkycodes/w2-tray-windows-macos`. Date 2026-11-23. Host: Fedora, Linux x86_64, Rust
1.95.0 stable. **No Windows and no macOS here**; everything Windows/macOS-specific is compile-only
and listed as unverified in `docs/architecture/gui.md` and `docs/dev/windows-verification.md`.

## Why this probe exists

`cargo check -p dblocal-gui --target x86_64-pc-windows-gnu` **cannot** run on this host: the
pre-existing `aws-lc-sys` build script needs `x86_64-w64-mingw32-gcc`, which is not installed
(the same limitation recorded in `docs/architecture/autostart.md` and
`docs/architecture/process.md`). `cargo build --release --target x86_64-pc-windows-gnu` also needs
`x86_64-w64-mingw32-dlltool` to link. Per rule 10 and the task, nothing is installed under `~/.cargo`
or as a system package to work around this.

So the Windows/macOS tray sources are type-checked in a scratch crate that holds **byte-for-byte
copies** of the real files and stubs only `dblocal-core`/`format` (the pieces that pull in the
uncross-compilable crypto stack). The `tray-icon`/`muda` calls being checked are the real ones.

## What is checked

| File | Origin |
|---|---|
| `probe/src/tray/mod.rs` | copy of `crates/gui/src/tray/mod.rs` |
| `probe/src/tray/spec.rs` | copy of `crates/gui/src/tray/spec.rs` |
| `probe/src/tray/desktop.rs` | copy of `crates/gui/src/tray/desktop.rs` (the subject) |
| `probe/src/tray/linux.rs` | copy of `crates/gui/src/tray/linux.rs` (unused on Windows) |
| `probe/src/format.rs` | stub: the `crate::format` functions the tray model calls |
| `probe/core-stub/src/lib.rs` | stub: `dblocal_core::{strings, model, service::InstanceView}` |

Refresh the copies after editing the real files, then:

```sh
cd docs/spikes/tray/probe

# 1. The copies must match the real sources, or this evidence means nothing:
for f in mod spec desktop linux; do
  diff -q "src/tray/$f.rs" "../../../crates/gui/src/tray/$f.rs" || echo "DRIFT: $f.rs"
done

# 2. Type-check the Windows backend against the real tray-icon + muda:
export CARGO_TARGET_DIR="$HOME/.cache/dblocal-dev/tray-probe-target"
cargo check --target x86_64-pc-windows-gnu        # -> Finished

# 3. macOS: the tray/muda/objc2 crates build, but std is missing.
cargo check --target aarch64-apple-darwin         # -> error[E0463]: can't find crate for `core`
```

The macOS target cannot be completed because no `*-apple-darwin` std is installed and rule 10
forbids installing one. To still exercise the identical `tray-icon`/`muda` Rust API,
`desktop.rs` also type-checks on **Linux** by adding `tray-icon` with its `ksni` feature to the
`cfg(target_os = "linux")` dependencies and marking `mod desktop;` reachable on Linux; then a plain
`cargo check` compiles `desktop.rs` against the real crate API (only the Windows/macOS-native
`MainThreadMarker`/`NSStatusBar`/`Shell_NotifyIconW` arms are specific to those targets).

## What the probe caught (real defects, fixed before merge)

Without the probe these would have shipped, since a Linux build never compiles `desktop.rs`:

1. `muda::Menu` owns its children internally, so the `Vec<MenuItemKind>` handles were dead; and
   `MenuItemKind` is **not** `IsMenuItem`, so `menu.append(&kind)` does not compile. Fixed by
   appending concrete `IconMenuItem`/`PredefinedMenuItem`.
2. `tray_icon::menu::Icon` (muda) is a **distinct type** from `tray_icon::Icon`; menu-row icons and
   the tray icon need separate constructors. Fixed.
3. `tray_icon::Error` has no `From<BadIcon>`; the `BadIcon` is now mapped to an `io::Error`. Fixed.

The Linux type-check (second route) and the Windows cross-check both pass after these fixes.

## Dependency evaluation (full table in `docs/architecture/gui.md`)

`tray-icon` 0.26.1 + `muda` 0.21.2 chosen over `windows-sys`/`objc2` hand FFI (hundreds of unsafe
lines, `unsafe_code = "deny"`) and over `ksni` (no Windows/macOS backend).

- New crates: 5 (`tray-icon`, `muda`, `crossbeam-channel`, `dpi`, `keyboard-types`); lock 960 → 965.
- Licences: all MIT OR Apache-2.0 (`cargo deny check licenses bans` → ok).
- `default-features = false` drops the Linux GTK/AppIndicator backend, so no GTK/libappindicator
  system library is added.
- Linux release binary unchanged: 79,063,688 → 79,061,968 bytes (−1,720 B, noise), because the
  dependency is `cfg(any(windows, macos))`. Windows size delta not measurable (no `dlltool`).
