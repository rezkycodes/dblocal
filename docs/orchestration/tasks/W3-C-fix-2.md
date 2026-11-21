# W3-C fix round 2 — one macOS-breaking bug, a broken real-terminal test, and surviving mutations

Same branch `w3-open-client`. Rules in `docs/orchestration/tasks/_COMMON-RULES.md` apply (every claim pasted output; no edits outside `crates/core/src/clients/**`, `docs/architecture/clients.md`).

The reviewer verified round 1 independently at `40958eb` + main: `cargo fmt`, `clippy -D warnings`, `cargo test -p dblocal-core --lib` (669 passed), `cargo deny` are all green, and the core mechanism WORKS: a scratch test that builds a launcher script with `launcher_temp_file`, runs it through `SystemSpawner` and the real `ptyxis --new-window -- sh {script}` delivered `PGPASSWORD` to the client both with no ptyxis running and with another ptyxis instance already running, and the script deleted itself. Do not rework that. The problems are below.

## H1 (HIGH) — the secure-directory check fails on every macOS launch

`terminal.rs::libc_uid()` reads `/proc/self/status`, which does not exist on macOS (`cfg(unix)` covers macOS). It then falls back to `u32::MAX`, so `verify_secure_dir` always returns "wrong owner" and no macOS launcher script can ever be created. `nix` is already a `cfg(unix)` dependency of `dblocal-core`: use `nix::unistd::geteuid()` (check that the needed feature is enabled; if enabling it would change `Cargo.toml`/`Cargo.lock`, ask first). Make `verify_secure_dir` take the expected uid as a parameter (`verify_secure_dir_for(dir, uid)`) so the owner check is testable on Linux with a wrong uid.

## H2 (MEDIUM) — the CLI plan ignores the detected client's path

`plan.rs::cli_command` builds `PathBuf::from("psql")`, `"mysql"`, `"mariadb"`, `"valkey-cli"`, `"redis-cli"` and the launcher script does `exec 'psql' ...`. The GUI process's `PATH` is not the terminal shell's `PATH`; a client found at `/usr/pgsql-16/bin/psql` would not be the one that runs. Use `client.path()` (the absolute detected path) for every CLI client; update the golden plans.

## H3 (MEDIUM) — the real-terminal test is itself broken, and tests pollute the user's real runtime directory

`real_ptyxis_receives_password_via_script` uses a wrapper `term-wrap.sh` that runs `exec ptyxis --new-window -- "$@"`, while the plan already carries `--new-window -- sh {script}`, so ptyxis is asked to run a command called `--new-window`, the script never runs, and the test FAILS (`DBLOCAL_REAL_TERMINAL=1 cargo test -p dblocal-core --lib clients::tests::f1_terminal::real_ptyxis -- --ignored` panics at `f1_terminal.rs:260`). It also left a launcher script containing its test password in `$XDG_RUNTIME_DIR/dblocal`. Rewrite it without a wrapper: the fake client gets an absolute path (H2 makes that work), `TerminalChoice::new(<real ptyxis path>, TerminalKind::Ptyxis)`, run, wait, assert the client saw the password, then assert no `dblocal-open-*` file remains.

The unit tests write into the real `$XDG_RUNTIME_DIR/dblocal` of whoever runs them: the reviewer found `helper.sh`, `helper.txt` and `dblocal-open-test-947026.sh` there. Give the spawner and `materialize_launcher` an injectable directory (for example `materialize_launcher_in(dir, temp)` and `SystemSpawner::with_launcher_dir`), make every test use a `tempfile` directory created with mode 0700, and keep at most one test that uses the default directory and removes what it created.

## H4 (HIGH) — five of nine mutations survive

Mutations run by the reviewer one at a time on a scratch copy (each build + `cargo test -p dblocal-core --lib clients`):

```
A password pushed into the terminal argv     KILLED (12 tests fail)
B drop shell_quote around the password       SURVIVED  (56 passed, 0 failed)   <- no test uses a hostile password
C fixed script file name                     KILLED (two_concurrent_launches_...)
D create_new(true) -> create(true).truncate  SURVIVED
E allow identifiers starting with '-'        KILLED (open_target_rejects_hostile_fields)
F disable the 10-minute sweep                SURVIVED
G skip the directory mode check              SURVIVED
H accept any host                            KILLED
I skip the directory owner check             SURVIVED
```

Add tests until all nine are killed by a NAMED test:
- B: run the generated POSIX launcher script with `sh` against a fake client that writes `$PGPASSWORD` to a file, over a password corpus: `it's`, `a"b`, `$HOME`, `$(id)`, backticks, `;`, `&&`, spaces, a backslash, `%`, `*`, a leading `-`, a very long password (4096 bytes), non-ASCII, and a newline; the file content must equal the password byte for byte for every entry.
- D: writing the same file name twice must fail the second time (give the writer an injectable file name or call the inner function twice).
- F: create an old script (use `File::set_modified` to 11 minutes ago) and a fresh one in a temp launcher directory; after `materialize_launcher_in` only the old one is gone and unrelated files are untouched.
- G and I: a directory with mode 0755 is refused; a directory that is a symlink is refused; `verify_secure_dir_for(dir, wrong_uid)` is refused; a correct 0700 directory owned by the current uid is accepted.

Paste the red output for B, D, F, G, I (and rerun A, C, E, H) and restore; `git status --short` must be clean at the end. Run mutations on a scratch copy or between commits, never leave a mutated file in the tree.

## H5 — coverage and cleanup

- Coverage in your own report: `plan.rs` 76.35%, `terminal.rs` 73.47%: below the 80% bar. The tests above should raise them; paste the new per-file table (`CARGO_LLVM_COV_TARGET_DIR=$HOME/.cache/dblocal-target-cov-w3c`, delete it afterwards; pure logic at 90% or more).
- Remove the `#[allow(dead_code)] script_placeholder_path` leftover.
- `clients.md`: state the exact guarantee after this round: the script exists on disk between creation and the terminal starting it (owner-only, 0600 in a 0700 directory, random name), is deleted by its own first command, swept after 10 minutes at the next launch; list which terminal command forms were verified on this machine (ptyxis only, with and without a running instance) and which are from documentation.

## Bar

`cargo fmt --all -- --check`, `cargo clippy -p dblocal-core --all-targets -- -D warnings`, full `cargo test -p dblocal-core --lib` x40, `cargo deny check`, `cargo xtask ci` all pasted; `DBLOCAL_REAL_TERMINAL=1 ... -- --ignored` passes on this machine and you paste it. `worker_done` only with pasted evidence; otherwise `ask`.
