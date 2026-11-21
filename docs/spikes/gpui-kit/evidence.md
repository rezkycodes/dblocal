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

## Release artifacts (same profile, both trees)

```
60207104  release/dblocal             (direct deps: 58685208)  -> +1.52 MB
60823448  release/examples/measure    (direct deps: 59311840)  -> +1.51 MB
```

## RSS — same-method A/B (the authoritative comparison)

`examples/measure` built from `main @ b7b18a7` (direct) and this branch (umbrella). The two
`measure.rs` sources differ only in import paths. Alternating A B A B A B, 3 reps each, one
session, no other build/test running.

```
          direct mean [min-max]        umbrella mean [min-max]      delta
FLOOR     176.8 MB [176.6-176.9]       177.3 MB [177.2-177.5]       +0.5 MB
IDLE      188.0 MB [187.9-188.1]       188.7 MB [188.6-188.8]       +0.7 MB
AFTER     188.2 MB [188.1-188.3]       188.9 MB [188.8-189.0]       +0.7 MB
CPU idle  0.08 %                       0.12 %                       +0.04 pp
CPU floor 0.13 %                       0.09 %                       -0.04 pp
```

Reconciliation: today `main`'s own binary reports FLOOR 167.6 / IDLE 180.0; the paired run above
reports 176.8 / 188.0 for the same direct edge. Absolute RSS moves up to ~8 MB between sessions,
so `gui.md`'s 165.4 / 178.6 (W1-G) is a historical point, stale by ~1-2 MB today, and cross-session
deltas at the 1 MB level are not meaningful. The +12.8 MB in the earlier draft of this report
compared different methods and is withdrawn, as is the untested "X11 code and its atlas" cause.

## Cold start — same probe, interleaved 20 runs per edge (Wayland)

Spawn -> `FIRST_FRAME_MS`, real shell window, 5 s exit. Single shared probe body; only the
dependency-edge module differs.

```
direct (pre-migration)  p50=494.2  p95=561.3  mean=501.5  min=448.6  max=571.7  n=20
umbrella (gpui-kit)     p50=489.1  p95=530.4  mean=493.7  min=449.4  max=538.8  n=20
```

X11 (umbrella, earlier run): p50 383.7 ms, p95 384.2, n=20. W0-C's 361 ms is a different method/machine
load and is not used as a baseline here.

## X11 runtime proof

```
INFO x11: compositor present: true, gtk_frame_extents_supported: true
DEBUG configuring surface with SurfaceConfiguration { format: Bgra8Unorm, width: 1180, height: 760, ... }
INFO Selected GPU (passed configuration test): AMD Radeon Graphics (RADV RENOIR) (Vulkan)
FLOOR: VmRSS=167.6 MB VmHWM=167.6 MB CPU=1.13%
INFO FIRST_FRAME_MS=0.06
```

## gpui-fast — same-probe A/B (floor and shell idle)

Both edges built from the identical shared probe body; 60 s idle sample; alternating.

```
floor  umbrella  mean=181.9 MB [181.4-182.3]      fast mean=184.1 MB [183.5-184.6]   +2.2 MB
shell  umbrella  median=184.8 MB [183.0-185.8]    fast median=187.5 MB [184.5-188.7] +2.7 MB
        idle CPU umbrella 0.06 %                   fast 0.05-0.08 %
```

Caveat: zram swap was fully exhausted (8191/8191 MB) by other workers during these runs, so some
samples show 60-155 MB after page reclaim. Medians above use only un-reclaimed readings (>=180 MB).
The earlier single-harness figures (default 180.5 / fast 183.6 MB) came from different harnesses and
are superseded by this same-probe comparison.

Probe binaries (real shell, stripped, lto=thin): default 45,842,472 B; gpui-fast 48,779,064 B.
