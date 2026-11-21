# gpui-fast spike notes

## Versions / license
- `cargo info gpui-fast`: 0.1.17, Apache-2.0, repo https://github.com/longbridge/gpui-fast
- `cargo info gpui-kit`: 0.10.0, Apache-2.0, features include `gpui-fast` (selects gpui-fast 0.1.16 in kit deps; gpui-fast 0.1.17 is latest published)
- Linux platform crate `gpui-fast-linux 0.1.17` defaults to `["wayland","x11"]` with `xkbcommon/x11` on x11 feature (same shape as gpui-pre-linux).
- This host lacks `libxkbcommon-x11.so` dev symlink; a Wayland-only probe (`default-features=false, features=["wayland"]`) compiles fine.

## What I could measure
- Upstream (gpui-pre 0.3.12) shell RSS/CPU/cold-start: measured on this host using the release `measure` example / existing baseline.
- gpui-fast full DBLocal shell A/B: **not measured** on this host because `gpui-kit`'s `gpui-fast` feature and the Fast stack pull the same x11 default, and building the DBLocal shell against Fast would require a separate Cargo manifest / patching platform deps (non-trivial and would touch build wiring outside owned paths). A minimal wayland-only empty-window probe compiles, but it is not the same shell and would not answer "do render tests still pass / pixel diff?" for the real app.
- Therefore gpui-fast is **not adopted**, and the recommendation is "needs more evidence" with a clear path: run the same measure+render suite on a host with xkbcommon-x11 dev (or a wayland-only build of the gpui-fast-linux stack) and compare against the numbers below.

## Windows/macOS
Not verifiable on this Fedora/Wayland host. gpui-fast docs advertise cross-platform; I did not build or run on Windows/macOS.
