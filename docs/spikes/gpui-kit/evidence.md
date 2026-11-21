# Evidence — W2-D GPUI Kit spike (2026-11-21)

Host: Fedora 44, kernel 7.3.0-cachyos1, GNOME 50.6, Wayland `wayland-0`,
AMD Radeon (RADV RENOIR). Toolchain rustc/cargo 1.95.0, edition 2024.
All commands run with `CARGO_TARGET_DIR=$HOME/.cache/dblocal-target-gui`.

## Umbrella-crate link failure (the blocker)

`cargo check -p dblocal-gui --all-targets` after a full import migration → **Finished** (green).

`cargo test -p dblocal-gui --no-run` → link error:

```
rust-lld: error: unable to find library -lxkbcommon-x11
collect2: error: ld returned 1 exit status
error: could not compile `dblocal-gui` (test "render") due to 1 previous error
error: could not compile `dblocal-gui` (lib test) due to 1 previous error
```

Host library state:

```
$ ls /usr/lib64/libxkbcommon-x11.so*
/usr/lib64/libxkbcommon-x11.so.0 -> libxkbcommon-x11.so.0.13.1
/usr/lib64/libxkbcommon-x11.so.0.13.1
$ ls /usr/lib64/pkgconfig/xkbcommon*.pc
(none)
$ cc probe.c -lxkbcommon-x11
/usr/bin/ld.bfd: cannot find -lxkbcommon-x11: No such file or directory
```

Cause: `gpui-kit 0.10.0`'s desktop platform dependency enables
`["font-kit","x11","wayland","runtime_shaders"]`; `gpui-pre-linux`/`gpui-fast-linux`
map `x11 → xkbcommon/x11`. Downstream cannot disable it.

## Dependency / lock check (final tree unchanged)

- `Cargo.lock`: 948 `[[package]]`, 824 unique `name =`; **byte-identical to HEAD**
  (`git diff HEAD --stat Cargo.lock` empty).
- No Cargo.toml change landed. No new dependency. `deny.toml` unchanged.
- `cargo deny check` → `advisories ok, bans ok, licenses ok, sources ok` (exit 0).

## Release artifacts (stripped, lto=thin)

```
58685208  release/dblocal
59311840  release/examples/measure
```

## OPEN-08 RSS / CPU (release `measure`, this host)

```
FLOOR:          VmRSS=167.5 MB VmHWM=167.5 MB CPU=0.20%  (target <150 MB, <1%)
IDLE:           VmRSS=167.7 MB VmHWM=178.2 MB CPU=0.15%
AFTER_DOWNLOAD: VmRSS=168.0 MB VmHWM=178.2 MB CPU=0.00%
```

(An earlier run of the same binary: FLOOR 160.7 MB, IDLE 167.7 / 178.2, AFTER 168.0.)

## Quality gates (final tree)

```
$ cargo fmt --all -- --check                  # exit 0
$ cargo clippy -p dblocal-gui --all-targets -- -D warnings   # Finished, 0 warnings
$ cargo test -p dblocal-gui --lib             # 83 passed; 0 failed
$ (x40 loop) cargo test -p dblocal-gui --lib  # ALL_40_OK (40/40 pass)
$ cargo deny check                            # advisories/bans/licenses/sources ok
$ cargo xtask ci                              # "xtask ci: all gates passed"
```

Render PNGs under `docs/design/renders/`: sha256 list identical before/after
(`RENDERS_UNCHANGED`). No code change was landed.
