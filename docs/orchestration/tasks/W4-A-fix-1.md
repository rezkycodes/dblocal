# W4-A fix 1 — three defects the reviewer found with the real binary, and one untested data-loss path

W4-A is merged into main (`43e7f8f`). New branch from main (`git checkout -b rezkycodes/w4-service-admin-2 main`), same worktree is fine. Rules `docs/orchestration/tasks/_COMMON-RULES.md` incl. 11-14. Do not touch `crates/gui/**`.

## What the reviewer verified and accepts (do not redo)

fmt, clippy (also `--features demo`), deny, core lib x3 (1186), service_admin x40 from a private copy (40/40), mock tests x3 (66, they only exist with `--features mock`), cli x3 (115), gui lib x3 (232), your real-PostgreSQL service e2e 5/5 under the e2e lock on my side. Real binary on an isolated home with a real PostgreSQL 18.7: `db list/create/drop` (identifier allowlist rejects `app;drop`, `a b`, empty, `x"y`, non-ASCII, `postgres`; a capitalised reserved word is created quoted), `user create` ro (SELECT ok, INSERT `permission denied`) and rw (INSERT ok), `password reset` (old password fails, new works, `dblocal uri` equals the printed URI), `db drop` with a missing confirm, a wrong confirm, `--confirm`, `--yes`, a missing database (`DBL-8105`) and a database with an open connection (`DBL-8106`), `open --dry-run` prints no password. Mutations: 6 of 7 red (typed-name check removed, wrong confirm accepted, half-created role kept, engine gate removed, package-manager apply allowed, `AdminChanged` never emitted).

## F1 (MEDIUM) — the credential pointer after a reset is not tested (mutation green)

Replacing `inner.db(|store| store.set_credential(&reference))?;` in `admin_ops::reset_superuser_password` with a no-op keeps every test green, including `reset_success_persists_the_new_pointer_and_announces`. That line is the whole point of F-DBU-03 when the keychain is unavailable: the new password lands in the FILE backend, the store pointer must follow it, otherwise the engine has the new password and DBLocal reads the old backend (an instance DBLocal can no longer log in to). Do: a test whose secret store reports a different backend for the write than the one recorded before (keychain pointer, file write), assert the pointer now says file and that `connection_uri` returns the new password; red against the mutation on the current head (paste), restored, clean `git status --short`.

## F2 (MEDIUM) — `user create` with an unknown database says the server is unreachable

`dblocal user create pg1 u9 --db nosuchdb --level ro` prints `[DBL-8100] Tidak dapat terhubung ke server database. Pastikan server sedang berjalan.` while the server is running (the grant step connects to the missing database and the connect failure is mapped to 8100). `db drop` already has `DBL-8105` "Database tidak ditemukan". Check that the database exists BEFORE creating the role (or map SQLSTATE 3D000 on that connect), report `DBL-8105`, and keep the existing guarantee that no role is left behind (I verified none is: your rollback works). Test at service level with the fake executor and once in the real-PostgreSQL e2e.

## F3 (LOW-MEDIUM) — a PostgreSQL instance created with `--no-password` says "engine not supported"

`dblocal create --engine postgresql --no-password` then `dblocal db list x` and `dblocal password reset x` print `[DBL-8111] Operasi ini belum didukung untuk engine tersebut.` The engine IS PostgreSQL; the reason is the missing password (`executor_for` gates on `auth() != Password`). Give that case its own typed, stable-coded Indonesian message that says what to do (the instance has no password; admin needs an instance created with a password), distinct from the MySQL/MariaDB/Valkey "not supported yet" error. One test per case.

## Open questions to answer in your report (no code unless trivial)

1. PRD F-DBU-02 AC2: "password-nya ditampilkan sekali **dan disimpan di keychain seperti kredensial superuser**". `user create` shows it once and stores it nowhere (`crates/cli/src/commands/user.rs` says so). The store has one credential row per instance (`credentials.instance_id` is the key). Say what storing it would cost (schema change to key by instance and user, `CredentialRef` shape, delete/purge paths, the migration) and recommend; I will take it to the user as a PRD deviation, do not implement it in this round.
2. `admin_ops` calls take no per-instance guard: a `password reset` running while a `stop`/`delete` of the same instance is in flight is not excluded. Say whether that can corrupt anything (secret removed mid-reset, engine stopped mid-statement) and what the smallest fix is (a read-only guard kind next to `LockKind`, or refuse when `is_busy`).
3. `dblocal update check` against the real release API fails with `[DBL-8004] Gagal memeriksa pembaruan. Periksa koneksi lalu coba lagi.` because the repository named in the constants (`rezkycodes/dblocal`) does not exist (the repo is `rezkycodes/db-local`, private). The message blames the connection. Map an HTTP 404/403 from the releases API to its own message ("sumber rilis tidak ditemukan atau tidak publik") so the failure is diagnosable. Small, with a test on the fake fetcher.

## Bar

fmt, clippy workspace (also `--features demo`), deny, the new tests x40 (private copy of the test binary; remember the mock-module tests need `--features mock`), `cargo xtask ci` under the heavy lock on the final head, the real-PostgreSQL service e2e under the e2e lock, and a transcript of F2 and F3 on the real binary (isolated `DBLOCAL_HOME`, delete the instance with `--purge` afterwards, `pgrep -x postgres` must be 0). `worker_done` only with the red/green output pasted.
