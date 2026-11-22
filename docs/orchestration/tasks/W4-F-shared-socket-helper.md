# W4-F — one shared implementation of the short-socket fallback and the private runtime directories (MySQL and MariaDB)

Start only after W3-B1 and W3-B2 are both merged. New branch from `main`; you own `crates/core/src/engine/` helpers you add, `engine/mysql/socket.rs`, `engine/mariadb/socket.rs`, and the two `prepare_run` call sites in `instance/start.rs`. Rules: `docs/orchestration/tasks/_COMMON-RULES.md` incl. 11-14.

## Situation

Both adapters ended up with their own copy of: socket path limit (107 Linux, 103 macOS), run-dir socket versus fallback directory (`$XDG_RUNTIME_DIR/dblocal` then `/tmp/dblocal-<uid>`), `verify_secure_dir` (0700, our euid, not a symlink), the socket cleanup and the per-instance private `tmpdir`. Two copies drift; the reviewer already found different bugs in each.

## Deliverables

1. A single module (for example `engine/sockdir.rs`, name it as you like) used by both adapters; the old per-adapter copies deleted, not left as wrappers with their own logic. Public surface small and documented.
2. Behaviour to implement ONCE and test ONCE:
   - candidates are a pure function of `(Option<OsString> xdg_runtime_dir, uid)`; XDG unset, empty and set are tested without env mutation;
   - the first candidate that is both verifiable and whose final `<dir>/m<port>-<12 hex>.sock` fits the limit wins; if none fits, the error names every candidate and why it was rejected, plus both lengths and the limit;
   - cleanup deletes a path only if its parent is one of the candidates, the file name matches `m<port>-<12 hex>.sock` for THIS port, and `symlink_metadata().file_type().is_socket()`; anything else is logged with `tracing::warn!` and left alone (tests: regular file with a valid name, socket with a foreign name, socket of another port are kept; a real `UnixListener` socket with the right name is deleted);
   - `verify_secure_dir` refuses a symlink, a regular file, mode 0755/0770, a foreign uid (call it with `uid + 1`), and a pre-existing wrong-mode directory is never silently `chmod`ed;
   - `prepare_run` on EVERY start recreates and re-verifies the fallback directory and the private `tmpdir`, removes a stale socket left by a crash, fails with a clear `Engine` error when it cannot;
   - `socket=` and `tmpdir=` are read from `my.cnf` with a parser that handles no key, empty value, spaces and comments.
3. Both adapters' existing tests keep passing unchanged except for moves; the real-binary e2e that deletes the fallback directory and the `tmpdir` between stop and start passes for MySQL 8.4 AND MariaDB 11.x. Mutation: make `prepare_run` a no-op, drop the `is_socket` check, drop the fits-check on the candidate: each goes red (paste).
4. Non-Unix targets compile (`cargo check --target x86_64-pc-windows-gnu -p dblocal-core`, the target dir is `~/.cache/dblocal-target-windows-gnu`).

## Bar

fmt, clippy workspace (and `--features demo`), `cargo deny check`, `engine::mysql` and `engine::mariadb` lib x20, full core lib x4, `mysql_e2e` and `mariadb_e2e` x3 under the e2e lock at load < 8 (paste the load), `cargo xtask ci` under the heavy lock on the final head, coverage of the new module >= 90%.

## Added by the reviewer after merging W3-B1 (2026-11-22)

Two items found while reviewing W3-B1, both belong to this task because it owns the socket-fallback code of both adapters:

1. **MySQL e2e deletes the user's real fallback socket directory.** `crates/core/tests/mysql_e2e/main.rs` (`mysql_84_deep_socket_restart_after_volatile_dirs_disappear`, about line 678) calls `fs::remove_dir_all(&fallback_dir)` where `fallback_dir` is `$XDG_RUNTIME_DIR/dblocal` or `/tmp/dblocal-<uid>`: the shared, per-user directory with the sockets of every DBLocal MySQL/MariaDB instance of whoever runs the test, and of the other deep-socket test in the same binary when threads run in parallel. Apply the same fix as MariaDB E4 (`W3-B2-resume-3.md`): after `initialize`, rewrite the `socket=` line of the instance's `my.cnf` to a path inside a private short directory the test creates under `/tmp` (unique name, mode 0700), delete THAT directory, then run `prepare_run` and start. Audit every e2e for other `remove_dir_all` calls on a directory the test did not create.
2. **Test-only copy of production logic.** `crates/core/src/engine/mysql/socket.rs` has `#[cfg(test)] resolve_socket_path_with_single_fallback_dir` and `resolve_socket_path_with_fallback_dir`, a second implementation of the resolver that several tests call instead of the production `resolve_socket_path_for`. Delete them and drive the tests through the one shared production function (with an injected candidate list).
