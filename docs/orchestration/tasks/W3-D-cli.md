# W3-D — The command-line interface (F-CLI-01..03, `logs`)

Branch: `w3-cli` from `main`. Independent of the GUI wiring (W2-B) and of the core follow-up (W2-F2): you use only the public `DblService` surface through the real `LocalService`.
Read first, in order: `AGENTS.md`, `docs/architecture/service.md`, `docs/architecture/process.md`, `docs/architecture/secrets.md`, `docs/architecture/e2e.md`, then `docs/prd/dblocal/PRD.md`: **F-CLI** (§F-CLI-01..03, the command table in §8), `F-INS-01..05`, `F-CRD-01..03`, `F-LOG-01..02`, decisions D-02, D-16, D-18. `crates/cli/src/cli.rs` has the command surface (clap) as documented stubs and the exit-code contract (0 ok, 1 general, 2 bad arguments, 3 instance not found, 4 engine failed to start). Never open an image with the Read tool. **Commit within your first 30 minutes and after every step** (a lost session once cost 2.5 hours of uncommitted work).

## Goal

Make DBLocal usable from `package.json`, a `Makefile` and onboarding scripts: the same `dblocal` binary, `dblocal <command>` instead of the GUI, real behaviour for `list`, `status`, `start`, `stop`, `uri`, `create`, `delete`, `logs`.

## You own

`crates/cli/**` EXCEPT the `None => dblocal_gui::run()` branch of `run()` in `cli.rs` (the GUI entry; another worker may touch it: do not edit those lines), `docs/architecture/cli.md` (new), and a minimal additive edit to `crates/xtask/src/e2e.rs` to run a CLI e2e target. User-facing CLI text lives in `crates/cli/src/messages.rs` (new; do NOT edit `crates/core/src/strings.rs`, the GUI worker owns it). You may not edit `crates/core/**` or `crates/gui/**`: if the service lacks something you need, use `ask` with the exact change.

## Design (decide it, write it in `docs/architecture/cli.md`, keep it small)

- A command layer that is a library function over `&dyn DblService` plus injected `out`/`err` writers and an `is_interactive` flag, so tests drive it with the in-memory mock service (`dblocal_core::service::mock`, feature `mock` as a dev-dependency only) and capture stdout and stderr separately. `main` builds the real `LocalService::new(DblPaths::from_env()?, ServiceOptions::default())` and a tokio runtime. A failure to open the service is a clear Indonesian error and exit 1, never a panic.
- **stdout is data, stderr is everything else** (progress, warnings, diagnosis). `uri` prints the URI alone plus a newline, nothing else (F-CLI-03 AC1); `uri --env` prints the `.env` block built by the core builder (do not hand-build it). `uri` is the one command that may print a password (the user asked for it); `list`, `status`, `logs`, errors and `tracing` never print a secret. Prove it with tests.
- `list`: table of name, engine, version, port, status (aligned, no colours when stdout is not a terminal); `--json` is a stable documented array of objects with English keys. `status <name>`: prints the status, exit 0 only when `running` (F-CLI-01 AC2), 3 when the instance does not exist.
- `start`: blocks until healthy or a timeout (`--timeout <seconds>`, default 60), driven by the service event stream, not by sleeping and polling blindly; a failed start prints the DBL code, the Indonesian message and the diagnosis, exit 4. A start of an instance that is already running (or being started by another process) prints "sudah berjalan", waits until it is healthy and exits 0 (F-CLI-02 AC3: the loser of a race must not start a second engine; the guarantee is the store's compare-and-set that already exists, add a test with two service instances over one home using the mock or a shared store). `stop` likewise, with the escalation the service implements.
- `create --engine --version --name [--port] [--no-password]`: non-interactive, shows download progress on stderr (only when stderr is a terminal, plain lines otherwise), prints the URI on stdout when done (F-CLI-03 AC2). Unknown engine or version, name already used, port in use: bad arguments or the typed error with its exit code, never a stack trace.
- `delete <name> [--yes] [--purge]`: refuses without `--yes` when not interactive (exit 2); the default ARCHIVES the data and says where, `--purge` deletes (F-CLI-03 AC3); interactive sessions ask for a typed confirmation of the instance name.
- `logs <name> [-f]`: last lines (`--lines`, default 200) then, with `-f`, follows the file across rotation until Ctrl-C (exit 0 on SIGINT); uses the service/core log reader, not ad hoc file reads, if one exists.
- Exit codes exactly as the contract; every error maps through one table with tests. `--help` text is useful and in English or Indonesian consistently (pick Indonesian like the GUI, flags stay English).
- No timer or busy loop while waiting: block on events/futures.

## Tests and bar

- Unit tests of argument parsing, the error to exit-code table, the table and JSON formatters (golden strings), the confirmation logic, and the secret-never-printed property for every command except `uri`. Mutation-check the exit-code table and the `--yes` refusal (revert each, paste the red output next to the test).
- Integration tests of every command against the mock service. A real binary test (`CARGO_BIN_EXE_dblocal`) that runs `--version`, `--help`, a bad argument (exit 2) and a `list` over an empty temp `DBLOCAL_HOME` (exit 0, empty table, valid empty JSON array).
- A CLI e2e (`#[ignore]`, run by `DBLOCAL_E2E=1 cargo xtask e2e`): the real binary creates a real Valkey instance, `status` exits 0 after `start` and non-zero after `stop`, `uri` pipes into a real `redis-cli`/client connection, `delete --yes` archives. Ports 20000-29999 only, temp home, cleanup guard that kills engines by validated command line even on panic (see `crates/core/tests/support` for the pattern, copy the idea, do not edit that directory). Three consecutive runs, paste the summaries.
- Pure logic >= 90% line coverage and every other file >= 80% (`cargo llvm-cov`, paste numbers); files < 800 lines, functions < 50 lines, no `unwrap`/`expect` outside tests; `cargo xtask ci` and `cargo deny check` green; the cli tests 40 times in a row.
- F-CLI-02 AC2 (the GUI reflects a CLI state change within 2 s) is a GUI-side behaviour; do not build it. In the report say whether the service gives a second process a way to observe changes (events are per-process) so the reviewer can raise it.

## Rules for this machine

RAM and disk are tight. ONE build cache for the whole project: `CARGO_TARGET_DIR=$HOME/.cache/dblocal-target-gui` (the cli crate links the GUI, so the core-only cache would rebuild GPUI); one cargo build at a time; never leave an engine or a window running; report `df -h /` and `free -m` before and after. `/tmp` is wiped on reboot: keep nothing important there. Report with pasted `git log --oneline -5`, a clean `git status --short`, largest `wc -l`, and what you could not verify (Windows and macOS behaviour of stdout, colours, Ctrl-C). Finish with `worker_done`; send `ask` if blocked.
