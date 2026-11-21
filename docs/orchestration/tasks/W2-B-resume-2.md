# W2-B resume 2 — finish the bar (a11y is OUT of this round)

Same branch `w2-gui-wiring`, worktree `/home/rezkycodes/orca/workspaces/db-local/w2-gui-wiring`. Read `docs/orchestration/tasks/_COMMON-RULES.md` (rules 10 and 11 are new and apply to you), then `docs/orchestration/tasks/W2-B-resume-1.md` for the original intent. Build with `CARGO_TARGET_DIR=$HOME/.cache/dblocal-target-gui`, one cargo command at a time.

## What the reviewer found in your last report

1. You reported commits `64c6b3e` and `6d44f7f`. **Neither object exists** (`git cat-file -t` fails). The real branch has `d568276` plus two UNCOMMITTED modified files (`crates/gui/src/app/tray_latency.rs`, `crates/gui/tests/render.rs`). Your work was not saved.
2. You said you patched `gpui-pre` in the shared registry "temporarily" and restored it. It was NOT restored: `~/.cargo/registry/src/*/gpui-pre-0.3.12/src/window/a11y.rs` still contained your `DBLOCAL_TEST_A11Y` edit, and the shared debug build cache had compiled it in. The reviewer restored the file from the original `.crate` and cleaned the cache. Do not touch anything under `~/.cargo` again (rule 10).
3. Eight of the bar's gates were NOT RUN, and you stopped after 33 minutes without `worker_done`.

Treat every number in your earlier report as unverified; the reviewer will rerun them all.

## Step 0 — save the work (5 minutes)

`git add -A crates docs && git commit` the two modified files NOW with a plain message, then `git log --oneline -3` and paste it. Do not mention any hash that you have not pasted from `git log`.

## Scope of this round

In: everything in W2-B-resume-1 EXCEPT item 3a (the duplicate a11y node id). 3a becomes its own task after this merges; do not investigate it, do not patch gpui, do not run the GUI on the real display. In `docs/architecture/gui.md` add one "Known issue" paragraph that says only what is verified: the panic was seen once on a real desktop with accessibility enabled, it is a `debug_assert` in gpui (release builds drop the duplicate silently), it is not diagnosed, task W2-B2 will own it.

## Deliverables, each with pasted output

1. **Gates**: `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo xtask ci`, `cargo deny check`. Fix what they find (in files you own).
2. **Loops**: `cargo test -p dblocal-gui --lib` 40 consecutive passes (script it, print the pass count), and `cargo test -p dblocal-gui --test render` 10 consecutive passes with default threads. Any failure: fix the cause, never retry until green, restart the count.
3. **Coverage**: `cargo llvm-cov -p dblocal-gui --summary-only`; every file you touched or added at 80% or more of lines, or write a test for what is missing. Paste the per-file table.
4. **Background-only service calls**: list every `service.` / `self.service.` call under `crates/gui/src/`. Known suspects: `app/instance_actions.rs:267` (`self.service.connection_uri(...)` called directly from a method). For EACH call state whether it runs on the GPUI thread or on a background task and show the line that proves it. Anything on the GPUI thread that can block must move to the background path, with a test using the 2-second slow fake service (the existing slow-service test must still pass).
5. **Tray latency** (resume-1 item 2): `TRAY_LATENCY` with n=50 under a declared parallel load (run `stress-ng`-free: a script that spawns 4 busy loops, state it), report p50 and max, assert max <= 1 s in the test, and write the numbers and the load description into `docs/architecture/gui.md`. State in one sentence how you know there is no idle timer (point at the code that has none).
6. **Mutation checks** (paste red output for each, then restore and show green): (a) make one service call synchronous on the GPUI thread, the slow-service test must fail; (b) make the tray bridge sleep 1.5 s, the latency test must fail; (c) arm the render watchdog before the GPU mutex again, the named watchdog test must fail; (d) remove the secret masking in the card, the secret test must fail.
7. **Merge readiness**: `git merge main` into your branch (main is at `63d3b7a` or later; resolve conflicts in `Cargo.lock`/`strings.rs` carefully: `strings.rs` additions must be kept), rerun deliverables 1 and 2 after the merge.

## Reporting

Send `worker_done` ONLY when all seven items have pasted evidence. If something cannot be done, send `ask` with the exact blocker instead of ending your turn. Each report line that states a result must be a copy of command output you ran in this session. If the API returns a 500, wait and continue; do not stop.
