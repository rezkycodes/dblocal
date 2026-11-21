# `gpui-fast` spike (not adopted)

## Version, license, age

- `gpui-fast 0.1.17` — Apache-2.0, repo https://github.com/longbridge/gpui-fast. Latest published
  0.1.x (`cargo info gpui-fast`, 2026-11-21).
- Selected through `gpui-kit = { version = "=0.10.0", features = ["gpui-fast"] }`; the kit's own
  manifest pins `gpui-fast = 0.1.16` (0.1.x is compatible, `cargo update` may pick 0.1.17).
- Linux backend `gpui-fast-linux 0.1.17`: `default = ["wayland","x11"]`, `x11 → xkbcommon/x11`.

## Build

With `libxkbcommon-x11-devel` installed on this host the Fast stack resolves and builds. A probe that
opens the **real** `crates/gui` shell against `gpui-kit = {features=["gpui-fast"]}` compiled in
~5 min (release, strip, lto=thin). No `[patch]` or crate alias was needed; `use gpui_kit::*` is
unchanged. `wgpu` had to be pinned `=30.0.1` in the probe manifest (both stacks resolve 30.0.1).

## Measured — same probe and session as the default edge

Both numbers come from the **identical shared probe body** (only the dependency-edge module
differs), 60 s idle sample each, alternating, one session. Earlier drafts compared figures taken
with different harnesses; this table supersedes them.

| Metric | Default (`gpui-pre`) | `gpui-fast` | Target |
|---|---:|---:|---:|
| Floor VmRSS (empty window) | 181.9 MB (median of 3) | 184.1 MB (median of 3) | < 150 MB |
| Shell idle VmRSS (real shell + mock) | 184.8 MB (median, n=9) | 187.5 MB (median, n=7) | < 150 MB |
| Idle CPU (60 s) | 0.06–0.10 % | 0.05–0.08 % | < 1 % |
| Probe binary (real shell, stripped) | 45,842,472 B | 48,779,064 B (+2.9 MB) | — |
| Cold start p50, Wayland (20 runs) | 489–494 ms | ~535 ms | < 1500 ms |

Same-probe verdict: `gpui-fast` is **+2.2 MB on the empty-window floor and +2.7 MB on shell idle
RSS** versus the default backend in the same session — it moves *away* from the 150 MB target, not
toward it. Idle CPU is marginally lower (0.05–0.08 % vs 0.06–0.10 %), both far under 1 %.

Measurement caveat: the host's zram swap was fully exhausted (8191/8191 MB) by other workers during
these runs, so a minority of samples showed 60–155 MB after page reclaim. Those are not an idle
footprint; the medians above use only un-reclaimed readings (≥180 MB). The delta's sign and rough
size were stable across every run (earlier harness: +3.1 MB; same probe: +2.7 MB).

## Render parity / tests — not run, deliberately

- Render parity against `docs/design/renders/*.png` and the GUI lib suite 40× under Fast were
  **not run, because the RSS result made adoption moot**: Fast measured +2.7 MB *more* shell idle
  RSS than the default backend in the same session and did not approach the 150 MB target, so there
  was no adoption path for the result to gate.

## Windows / macOS

Not verifiable on this Fedora/Wayland host: `gpui-fast` advertises the same desktop targets and its
`gpui-fast-{macos,windows,apple}` crates are in the lock, but no build or run was possible.

## Recommendation

**Do not adopt.** `gpui-fast` builds and runs, but it does **not** reduce the idle RSS floor: with
the same probe it measured 2.2 MB higher on the floor and 2.7 MB higher on shell idle than the
default backend in one session (both above the 150 MB target), plus ~2.9 MB of binary and a slower
start. Render parity and 40× stability were not run because the RSS result left no adoption path.
Revisit only if a memory investigation specifically targets the retained-rendering structures and a
real-shell A/B (RSS, CPU, render pixel diff, 40× lib, Windows/macOS build) clears the bar.
