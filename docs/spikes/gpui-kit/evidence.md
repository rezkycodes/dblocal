# Evidence — W2-D GPUI Kit foundation (2026-11-21)

Host: Fedora 44, kernel 7.3.0-cachyos1, GNOME 50.6, Wayland `wayland-0` + Xwayland `:0`,
AMD Radeon (RADV RENOIR) selected; NVIDIA GTX 1660 Ti also enumerated. rustc/cargo 1.95.0.
All commands: `CARGO_TARGET_DIR=$HOME/.cache/dblocal-target-gui` (Fast probe:
`$HOME/.cache/dblocal-fast-target`).

## Umbrella crate

Build-time prerequisite now present: `pkg-config --exists xkbcommon-x11` → 1.13.1;
`/usr/lib64/libxkbcommon-x11.so -> libxkbcommon-x11.so.0`.

Before the package was installed, the same migration failed to link:

```
rust-lld: error: unable to find library -lxkbcommon-x11
collect2: error: ld returned 1 exit status
error: could not compile `dblocal-gui` (test "render") due to 1 previous error
```

After install: `cargo check -p dblocal-gui --all-targets` green; `cargo test -p dblocal-gui`
compiles and links.

Platform features the kit enables (from `cargo tree -e features -i gpui-pre-platform`):
`gpui-pre-platform` ← `default`(multithreaded), `font-kit`, `x11`, `wayland`, `runtime_shaders`
(all from `gpui-kit 0.10.0`); `gpui-pre-linux` `default = ["wayland","x11"]`.

## Tests / gates (final tree)

```
$ cargo fmt --all -- --check                          # exit 0
$ cargo clippy -p dblocal-gui --all-targets -- -D warnings   # 0 warnings
$ cargo test -p dblocal-gui
  lib:                    83 passed; 0 failed
  contrast_table:          1 passed; 0 failed
  render:                  5 passed; 0 failed  (45.28 s)
$ (x40) cargo test -p dblocal-gui --lib               # ALL_40_OK
$ cargo deny check                                    # advisories/bans/licenses/sources ok
$ cargo xtask ci                                      # "xtask ci: all gates passed"
```

Render PNG parity (committed vs regenerated in `target/`, `cmp` per file): **14/14 identical**.

## Release artifacts

```
60207104  release/dblocal             (was 58685208 direct-deps baseline)
60823448  release/examples/measure    (was 59311840)
```

## OPEN-08 numbers

Default (umbrella) stack, `examples/measure`:
```
FLOOR:          VmRSS=167.6 MB VmHWM=167.6 MB CPU=0.20%
IDLE:           VmRSS=180.5 MB VmHWM=180.5 MB CPU=0.25%
AFTER_DOWNLOAD: VmRSS=181.2 MB VmHWM=181.2 MB CPU=0.00%
```
Direct-deps baseline (before migration): FLOOR 167.5, IDLE 167.7/178.2, AFTER 168.0.

## Cold start (wall clock, process spawn → FIRST_FRAME_MS, 20 runs, single-window real-shell probe)

```
Wayland, default:  P50=464.8 ms P95=525.0 N=20 MIN=444.0 MAX=2059.1
X11, default:      P50=383.7 ms P95=384.2 N=20 MIN=363.2 MAX=404.2
Wayland, gpui-fast:P50=535.3 ms P95=749.9 N=20 MIN=464.2 MAX=2058.6
```

## X11 runtime proof

```
INFO x11: compositor present: true, gtk_frame_extents_supported: true
DEBUG configuring surface with SurfaceConfiguration { format: Bgra8Unorm, width: 1180, height: 760, ... }
INFO Selected GPU (passed configuration test): AMD Radeon Graphics (RADV RENOIR) (Vulkan)
FLOOR: VmRSS=167.6 MB VmHWM=167.6 MB CPU=1.13%
INFO FIRST_FRAME_MS=0.06
```

## gpui-fast shell idle (real shell, 60 s idle sample)

```
SHELL_IDLE: VmRSS=183.6 MB VmHWM=183.6 MB CPU=0.08%   # gpui-fast
```

Probe binaries (real shell, stripped, lto=thin): default 45,842,472 B; gpui-fast 48,765,032 B.
