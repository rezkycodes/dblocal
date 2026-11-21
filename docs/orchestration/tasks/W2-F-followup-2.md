# W2-F follow-up 2 — finish the gates you left open, strengthen M10, and one new core defect

Same branch (`rezkycodes/w2-f-followup-1`, you are on it, HEAD `77e2395`). Rules: `docs/orchestration/tasks/_COMMON-RULES.md` (rules 10 and 11: every claim pasted). The reviewer read your last summary: items 1-9 of `W2-F-followup-1.md` have code and tests, M7, M11 and M10 are red-then-green, and you honestly listed what is NOT done. You then ended your turn without `worker_done` or `ask`, so nothing reached the coordinator and the branch has been idle for four hours. Do not do that again: a report is either `worker_done` (all evidence pasted) or `ask` (a real blocker) and you keep working on everything unblocked.

## 1. The open gates (nothing else may be reported green before these are pasted)

"Shared target contention" is not a blocker: cargo waits for the build lock. Run every long gate in the background with a log file (`nohup bash -c '...' > $HOME/.cache/w2f2-<gate>.log 2>&1 &`), wait on it with an until-loop that checks the log (commands in the foreground are cut at 10 minutes), and paste the tail of the log. Use `CARGO_TARGET_DIR=$HOME/.cache/dblocal-target` for core gates.

- `cargo fmt --all -- --check`, `cargo clippy -p dblocal-core --all-targets -- -D warnings`, the FULL `cargo deny check` (not only `licenses bans`), `cargo xtask ci`.
- `DBLOCAL_E2E=1 cargo xtask e2e`: 15/15 on real PostgreSQL and Valkey, three consecutive runs (the engine/extract/process code changed, so this is the real regression check).
- The instance + extract library suites 40 times (`cargo test -p dblocal-core --lib instance download` in a loop; paste the pass count and any failing name).
- Coverage with its own cache, `CARGO_LLVM_COV_TARGET_DIR=$HOME/.cache/dblocal-target-cov-w2f2 cargo llvm-cov -p dblocal-core --lib --summary-only`; delete the cache afterwards. Every file you touched at 80% lines or more (pure logic 90%); `engine/postgres.rs`, `engine/valkey.rs` and `download/extract/tarball.rs` were below the bar before you started, so lift them or say with numbers what the gap is. Files under 800 lines; `instance/helpers.rs` is at 725, so watch it.

## 2. M10 mutation nuance

You reported: "the managed-block branch disabled (falls through to legacy/append)" survives the M10 test because the legacy rewrite produces the same effective lines. Strengthen the test (exact managed-block replacement against the real new-layout file: the BEGIN/END markers, the surrounding user lines and the line order are byte-identical afterwards except for the port line; the legacy append path would add a second block or a duplicate `port` line) and paste both mutants red. Also re-mutate M1, M2, M3 once each and paste, since you did not.

## 3. New item 10 (found by the reviewer on the real binary): no-password instances have no URI

For an instance created with `AuthMode::None`, `instance::read_ops::connection_uri` returns `DblError::NotFound { kind: "credential" }` because it requires a stored credential row. Real binary: `dblocal create --engine valkey --version 9.2.0 --name p --no-password` exits 3 with `[DBL-1002]` (the instance IS created and runs), and `dblocal uri p` / `uri p --env` exit 3. The mock service hides it because it always has a password. PRD F-CLI-03 AC2 and F-INS list `--no-password` as supported, so the URI must exist without a password:

- valkey: `redis://127.0.0.1:PORT/0`; postgresql: `postgresql://USER@127.0.0.1:PORT/DB` (no `:` and no empty password); the `.env` block likewise (no `PASSWORD=` line, or an empty one only if the existing golden shape requires it: say which and why).
- Fix in the core: when no credential row exists AND the instance's auth mode is `None`, render through the adapter without a password (`uri::render` needs a no-password path); keep every password-bearing output byte-identical (golden tests unchanged); when the auth mode is `Password` and the credential is missing, keep the typed `NotFound`/error.
- Tests: both engines x both formats x both auth modes; the missing-credential-with-Password case; and through the real `LocalService` (not the mock). Make the mock honour the same rule so a CLI/GUI test cannot hide it again (`service/mock/ops.rs`: it is core, in scope).
- Mutation: render `:` plus an empty password in the no-password branch, then a named test must fail. Paste red and green.
- Do not touch `crates/cli` or `crates/gui`.

## Bar and report

Everything in section 1 pasted, section 2 and 3 evidence pasted, `git status --short` clean, commits after each step. `worker_done` only then. If a gate fails for a reason outside your files, paste the failure and send `ask` while you continue with the rest.
