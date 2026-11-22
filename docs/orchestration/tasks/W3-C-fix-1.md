# W3-C fix round 1 — review findings on the open-in-client module

Same branch `w3-open-client`. Read `docs/orchestration/tasks/_COMMON-RULES.md` (rules 10 and 11 are new: every fact in your report must be pasted output). Finish what you are doing, then do this round. The reviewer tested commit `051878a`, in which `cargo fmt --all -- --check` FAILED (diffs in `clients/tests/mod.rs`, `run_extra.rs`, `terminal_extra.rs`) although the report said it passed. Re-run it at the end.

## F1 (HIGH) — an environment variable on the terminal process does NOT reach the client

Evidence, run by the reviewer on this machine (Fedora 44, the only terminal installed is `ptyxis`):

```
# no ptyxis running:                 DBL_CANARY=x ptyxis --new-window -- sh -c 'echo $DBL_CANARY'   -> x
# a ptyxis instance already running: DBL_CANARY2=y ptyxis --new-window -- sh -c 'echo $DBL_CANARY2' -> (empty)
```

Terminals that are single-instance servers (ptyxis, gnome-terminal, and possibly konsole/kitty with a single-instance option) hand the command to a running process that does not carry the caller's environment. So `clients.md`'s claim "Linux terminals that accept -e/-- argv can receive env vars through the spawned process environment" is false, and with a running terminal the password would never arrive (best case psql just prompts).

Fix: use ONE mechanism on every OS for CLI clients that need a password: a private launcher script, run by the terminal as `sh <script>` (cmd on Windows), instead of relying on env inheritance. Rules for it (one shared implementation, used by Linux, macOS and Windows):

- Directory: Linux `$XDG_RUNTIME_DIR/dblocal` (per-user tmpfs, already 0700; if unset fall back to `$HOME/.dblocal/run`), macOS `$TMPDIR/dblocal` (per-user), Windows `%LOCALAPPDATA%\dblocal\run`. Create with mode 0700; if it exists verify with `symlink_metadata`: a real directory, owned by the current uid, mode exactly 0700, otherwise refuse with a typed error (never `chmod` someone else's directory).
- File: random name (at least 128 bits from the same RNG the secrets code uses), created with `O_CREAT|O_EXCL|O_NOFOLLOW`, mode 0600, content written, `sync_all`. Never a fixed name: today two quick "Open in…" clicks overwrite each other's script (`dblocal-open-client.sh`).
- The path used in the terminal's command line and the path the spawner writes to must come from ONE function. Today `macos_launcher_script_path` hardcodes `/tmp/dblocal-open-client/...` while the spawner writes under `std::env::temp_dir()` (a different directory on macOS), so the macOS command would run a file that does not exist. Keep `plan_open` pure: the plan carries a placeholder (for example `{script}`), the spawner substitutes the real path. Test: with `TMPDIR`/`XDG_RUNTIME_DIR` pointing at a temp dir, the path inside the spawned command equals the file that was written.
- The script deletes itself as its FIRST action (POSIX: `rm -f -- "$0"`), then exports the variable and `exec`s the client. If the spawn fails, the spawner deletes the file. At each launch the spawner also deletes files older than 10 minutes in that directory (a terminal that never started would otherwise leave a password on disk).
- Windows batch caveats, from known cmd.exe behaviour (cannot be run here, so keep the checklist row): cmd re-reads a `.cmd` file between commands, so a `del "%~f0"` on its own first line makes the following lines fail; put the delete, the `set` and the client call inside ONE parenthesised block `( ... )`, which cmd parses completely before running. Also `%` must be doubled to `%%` in every value written into the script (a password containing `%PATH%` is expanded today: `escape_cmd_set_value` does not handle it). Reject passwords containing `"`, CR or LF on Windows with a typed error instead of writing a broken script.

Terminals: add `xdg-terminal-exec` (first, if present; it is the freedesktop standard and this machine's GNOME setting points at it), `ptyxis` (`ptyxis --new-window -- sh <script>`; `-e` is NOT an option of ptyxis), `foot`, `wezterm` (`wezterm start -- sh <script>`) next to the existing ones. Check each tool's real flag in its man page or `--help` where installed and say which forms you verified on this machine and which are from documentation only (kitty takes the program as positional arguments: confirm `-e` before using it). The recorded order must be in the doc. `$TERMINAL` must be checked for existence. When nothing is found return a typed `NoTerminal` error (Indonesian message), not a fallback to `xterm` that fails later as "client xterm not found".

A real test on this machine with a FAKE terminal (a shell script in a temp dir that ignores its own environment, like a server terminal, and just runs `sh "$last_arg"`): the client (a fake `psql` script writing `$PGPASSWORD` and `/proc/self/cmdline` of its parent chain to a file) receives the password; the password is in NO process's `/proc/<pid>/cmdline` and NO `/proc/<pid>/environ` of the terminal process; the script file no longer exists afterwards; two concurrent launches produce two different files and each client gets its own password. Plus an `#[ignore]` test that runs the same through the real `ptyxis` when `DBLOCAL_REAL_TERMINAL=1`; the reviewer will run it.

## F2 (HIGH) — option and connection-string injection through the target fields

- `mysql`/`mariadb` get the database as a trailing positional argument (`plan.rs`, `target.database().to_owned()` last in argv). A database named `--init-command=...` or `-e...` is parsed as an option. Use `--database=<value>` (a single token, so the value can never start an option).
- `psql --dbname <value>`: libpq treats a dbname that contains `=` or starts with `postgresql://` as a full connection string, so `x host=attacker.example` would send the connection AND `PGPASSWORD` to another host (documented libpq behaviour; psql is not installed here so this is from the docs, not run).
- The doc says "a leading `-` is safe when passed as argv": that is false for positional arguments.

Fix at the boundary: `OpenTarget::new` returns `Result` and validates: host must be loopback (`127.0.0.1`, `::1`, `localhost`: DBLocal never listens elsewhere); username and database `[A-Za-z0-9_.-]`, at most 63 characters, not starting with `-` (database may be empty); instance name: no control characters. Typed error with the module's code. Then the hostile corpus is asserted at BOTH layers: construction rejects it, and (for the fields that remain legal) the plan never produces an argv element that starts with `-` other than the fixed flags.

## F3 (MEDIUM) — TablePlus mechanism is not the documented one

The official TablePlus docs describe opening a connection with its URL form `postgresql://user@host:port/db` (values URL-encoded; "Copy as URL"; `open -a TablePlus "<url>"` on macOS): https://docs.tableplus.com/gui-tools/manage-connections . The plan uses `tableplus://?name=...&driver=...`, which is not in the official docs (the PRD's "e.g. `tableplus://`" was an example, not a verified form). `xdg-open` on Linux routes by scheme and may open a different application. Change to the documented URL form WITHOUT password, macOS `open -a TablePlus <url>`, Linux/Windows: pass the URL to the DETECTED TablePlus executable path. `mysql://` and `redis://` forms are only attested by third-party reports: mark them unverified in the doc matrix. Put the doc URL in `clients.md`.

## F4 (MEDIUM) — DBDelve and DBeaver arguments

- DBDelve: `--driver --host --port --user --database --name` are invented (nothing documents them; `clients.md` even says "a connection URI argument"). A DBDelve binary exists on this machine (`~/.local/bin/dbdelve`); with no display `--help` prints nothing and the reviewer could not confirm any flag. Do NOT guess: the DBDelve plan launches the application only (its detected path, no connection arguments) and sets `copy_uri_hint = true` on the plan so the GUI also puts the password-less URI on the clipboard; `clients.md` says DBDelve's connection argument mechanism is unknown. If you can find the real mechanism in DBDelve's own help or docs on this machine, show the output and use it.
- DBeaver `-con` is documented (https://dbeaver.com/docs/dbeaver/Command-Line/): `driver`, `host`, `port`, `name`, `openConsole` are real parameters. The driver id for MariaDB (`maria`) is not confirmed there: mark unverified, and keep values that contain `|` rejected (already so).

## F5 (LOW)

- `SystemSpawner` sets `DBLOCAL_LAUNCHER_SCRIPT` and nothing reads it: remove.
- Spawner: do not silently drop the child's exit; keep the `wait` thread but name it.
- The doc's Secrets section must state the residual exposure truthfully per OS after F1 (the script exists on disk for the moment between creation and the terminal starting it; owner-only; deleted by the script first thing; swept after 10 minutes).

## Bar

`cargo fmt --all -- --check`, `cargo clippy -p dblocal-core --all-targets -- -D warnings`, `cargo test -p dblocal-core --lib clients`, `cargo deny check`, and `cargo xtask ci` all pasted. Coverage: run `cargo llvm-cov` with its own cache (`CARGO_LLVM_COV_TARGET_DIR=$HOME/.cache/dblocal-target-cov-w3c`, delete it when done) so that it does not collide with the shared one; paste the per-file table (pure logic >= 90%, everything else >= 80%). 40 consecutive passes of the clients tests, and the concurrency test 40 times.

Mutation checks, red output pasted for each, then restored and green: (a) put the password into argv; (b) remove `shell_quote` in the launcher body; (c) a fixed script file name; (d) drop `O_EXCL`; (e) drop the validation that rejects a database starting with `-`; (f) drop the 10-minute sweep. Run mutations on a scratch COPY or between commits, never leave a mutated file in the tree; show `git status --short` clean at the end.

Report (`worker_done`): what changed per finding, the pasted outputs above, and the unverified list: Windows script behaviour, macOS Terminal.app, each terminal whose flag you only read about, TablePlus/DBeaver/DBDelve real behaviour.
