# W4-A3 fix 2 — one ordering defect in the new Running gate (small)

Same branch and worktree. Rule 15. FIRST `git merge main` (main moved: W4-B merged; the only textual conflict is `docs/dev/windows-verification.md`, where main added three GUI rows (W4-B) next to your MySQL/MariaDB rows: keep BOTH, your three W4-A3 rows first, then the three W4-B GUI rows; the reviewer resolved it that way in his trial merge).

Reviewed and accepted from fix 1: the gate covers every admin call including the reset, `mysql_target` is pure and pinned (port and socket, MySQL and MariaDB), the wrong-engine arm is `Unsupported` again, the docs are true now.

## F4 (LOW) — a stopped Valkey instance now answers "start the server" instead of "not supported"

`executor_for` checks `status != Running` before the Valkey engine gate. `dblocal db list <stopped-valkey>` therefore says `DBL-8100 Pastikan server sedang berjalan`; after the user starts it, the answer becomes `DBL-8111 not supported`. The engine answer must not depend on the state: Valkey never has named databases or login roles. Reorder to engine gate (Valkey -> `Unsupported`) first, then the Running gate, then the password gate (still before the credential read and the factory). Test: a Stopped and a Running Valkey row both give `AdminError::Unsupported` / `DBL-8111` from every admin call, factory call count 0, no credential read. Mutation (paste red): the Running gate moved back in front of the engine gate.

## Bar

fmt, clippy workspace (also `--features demo`), deny, the new test x40 from a private copy, core lib x1, `service_e2e admin_mysql` is NOT needed again (no e2e path changed), `cargo xtask ci` under the heavy lock on the final head. `worker_done` with the red/green output pasted.
