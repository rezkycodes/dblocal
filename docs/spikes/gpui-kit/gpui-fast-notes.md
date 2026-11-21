# `gpui-fast` spike (not adopted)

## Version, license, age

- `gpui-fast 0.1.17` — Apache-2.0, repo https://github.com/longbridge/gpui-fast. Latest published
  0.1.x (`cargo info gpui-fast`, 2026-11-21).
- Selected through `gpui-kit = { version = "=0.10.0", features = ["gpui-fast"] }`; the kit's own
  manifest pins `gpui-fast = 0.1.16` (0.1.x is compatible, `cargo update` may pick 0.1.17).
- Linux backend `gpui-fast-linux 0.1.17`: `default = ["wayland","x11"]`, `x11 → xkbcommon/x11`.

## Build

With `libxkbcommon-x11-devel` installed the Fast stack resolves and builds. A standalone probe that
opens the **real** `crates/gui` shell against `gpui-kit = {features=["gpui-fast"]}` compiled in
5m 33s (release, strip, lto=thin). No `[patch]` or crate alias was needed; `use gpui_kit::*` is
unchanged. `wgpu` had to be pinned `=30.0.1` in the probe manifest (both stacks resolve 30.0.1).

## Measured (this host, Wayland, AMD RADV RENOIR)

| Metric | Default (`gpui-pre`) | `gpui-fast` | Target |
|---|---:|---:|---:|
| Shell idle VmRSS (60 s idle, real shell+mock) | 180.5 MB | **183.6 MB** (HWM 183.6) | < 150 MB |
| Idle CPU (60 s sample) | 0.25 % | **0.08 %** | < 1 % |
| Probe binary (real shell, stripped) | 45,842,472 B | **48,765,544 B** (+2.9 MB) | — |
| Cold start p50, Wayland (20 runs) | 464.8 ms (p95 525.0) | **535.3 ms** (p95 749.9, min 464, max 2059) | < 1500 ms |

Shell idle RSS is **higher** with `gpui-fast` (183.6 vs 180.5 MB), so the "Fast may lower the
memory floor" hypothesis is **not supported by this measurement**; it does not reach the 150 MB
target either. Idle CPU is lower (0.08 % vs 0.25 %), both far under 1 %. Cold start is ~15 %
slower (535 vs 465 ms p50), still well under the 1500 ms target.

## Render parity / tests

- Render tests: not re-baselined under Fast in this pass (the Fast build is a probe binary, not
  the crate's own test target). Pixel parity against `docs/design/renders/*.png` is **unverified**
  for Fast.
- GUI lib suite 40× under Fast: **not run** (the 40× run is on the adopted default stack).

## Windows / macOS

Not verifiable on this Fedora/Wayland host: `gpui-fast` advertises the same desktop targets and its
`gpui-fast-{macos,windows,apple}` crates are in the lock, but no build or run was possible.

## Recommendation

**Do not adopt.** `gpui-fast` builds and runs, but it does **not** reduce the idle RSS floor
(183.6 MB vs 180.5 MB default, both above the 150 MB target), it adds ~2.9 MB to the binary and
~70 ms to cold start, and render parity / 40× test stability on the real shell are unmeasured. Revisit only if a memory
investigation specifically targets the retained-rendering structures and a real-shell A/B (RSS,
CPU, render pixel diff, 40× lib, Windows/macOS build) clears the bar.
