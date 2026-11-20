# Spike: build Valkey for Windows and macOS (F-SPK-02)

**Question:** can we build `valkey-server` from source with CMake on `windows-latest`, macOS arm64
(`macos-15`) and Intel macOS (`macos-15-intel`), start it, get `PING` → `PONG`, and shut it down
cleanly?

**Status: UNVERIFIED.** The workflow `.github/workflows/valkey-build.yml` is written but has never
run on GitHub Actions. Everything below is the checklist for the first real run, not a claim about
the outcome.

---

## Why this spike exists

Per decision **D-13**, Valkey is the Redis-compatible engine. Linux binaries come from the official
`download.valkey.io` tarballs (with SHA-256). Windows and macOS have no official binaries, so this is
a deliberate, narrow exception to D-12 ("don't build engine binaries ourselves"): we need to build
Valkey in CI for those two platforms.

If it fails on a platform, the fallback (D-13 AC3) is to mark Valkey `unsupported` there in the
catalog, show the reason + a suggestion in the UI, and update the PRD. That is a first-class
outcome, not a bug.

## How to run it

Actions → **Valkey build (spike)** → *Run workflow*, with `valkey_tag` set to the release you want
(e.g. `8.1.6`, `9.0.3`). The job matrix is `windows-latest`, `macos-15`, `macos-15-intel`.

## What to look at

For each platform, the workflow:

1. **Checks out the pinned tag** — confirms the tag exists and is a real release, not a moving
   branch tip.
2. **Configures with CMake** (`cmake -S . -B build -DCMAKE_BUILD_TYPE=Release`) — the key question is
   whether upstream's CMake project configures cleanly on Windows (MSVC) and both macOS
   architectures with the runner's default toolchain, no extra flags.
3. **Builds** (`cmake --build build --config Release --parallel`) — record **wall-clock build time**
   and **peak disk** usage; a multi-hour or 20 GB build would make per-release rebuilds unattractive
   and push us toward prebuilt-once-per-version caching.
4. **Locates `valkey-server` and `valkey-cli`** — the `locate` step guesses the paths
   (`build/Release/…` on Windows, `build/src/…` on Unix, plus `redis-server` aliases). **This is the
   step most likely to need fixing.** If it fails, the step dumps `find build -type f` so the real
   output paths can be read from the log and the lookups adjusted.
5. **Smoke-tests** — starts `valkey-server` on `127.0.0.1:46379`, waits up to 30 s for readiness,
   asserts `PING` returns `PONG`, then `SHUTDOWN NOSAVE` and confirms the process exits (force-kill
   only as a fallback, and that is reported loudly). This is the actual AC1/AC2 evidence.
6. **Uploads** `valkey-server`, `valkey-cli`, `VALKEY_TAG` and `BUILD_PLATFORM` as an artifact named
   `valkey-<tag>-<os>`.

## Questions to answer on the first run

- [ ] Does CMake configure on **Windows MSVC** without extra generators or flags?
- [ ] Are binaries produced where `locate` expects them (adjust if not)? Are they named
      `valkey-server`/`valkey-cli`, or only `redis-server`/`redis-cli` (the aliases the workflow
      also accepts)?
- [ ] Does `valkey-cli` build by default on Windows, or does CMake skip it?
- [ ] Build time and disk usage per platform — acceptable per release, or do we cache?
- [ ] Do the produced binaries depend on system libraries absent on a plain user machine (especially
      macOS: Homebrew `openssl`/`libssl` linkage)? `otool -L` the macOS binaries.
- [ ] On macOS, are the binaries arm64 / x86_64 as expected per runner?
- [ ] Does `SHUTDOWN NOSAVE` exit cleanly within the grace window on all three platforms?
- [ ] Any code-signing requirement to run the Windows binary (none expected for a CLI server, but
      confirm).

## Recording the result

After a run, replace this section with a dated table:

| Platform | Configure | Build | PING | Clean shutdown | Notes |
|---|---|---|---|---|---|
| windows-latest | ? | ? | ? | ? | |
| macos-15 (arm64) | ? | ? | ? | ? | |
| macos-15-intel | ? | ? | ? | ? | |

Then either (a) keep D-13 as-is and wire these artifacts into the catalog build, or (b) trigger the
D-13 fallback for any platform that fails, updating `docs/prd/dblocal/PRD.md` and the catalog
`unsupported` reason.

## Open risks

- Upstream Valkey supports Linux primarily; Windows is via CMake but not an officially exercised
  target, so this may simply not work without patches — the D-13 fallback exists for that reason.
- macOS binaries built on `macos-15`/`macos-15-intel` target the runner's SDK; confirm they run on
  the oldest macOS we support (13+).
- Reproducibility: the workflow checks out a tag, which is good, but the toolchain (CMake, compiler)
  varies by runner image over time; pin or record the image versions used for a release.
