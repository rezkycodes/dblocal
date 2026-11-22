# W4-A — put admin, update and open-in-client behind `DblService`, and into the CLI (F-DBU, F-OPN, F-PKG-05 on the real service)

Start only after W2-A (instance service flake hunt) is merged: you add methods to `crates/core/src/service/mod.rs`, `instance/service.rs` and `service/mock.rs`. New branch from current `main`, worktree under `~/orca/workspaces/db-local/`. Rules: `docs/orchestration/tasks/_COMMON-RULES.md` incl. 11-14. You do NOT touch `crates/gui/**` (W4-B does the screens after this merges and after W2-B).

## Situation

Three finished libraries are not reachable by any front-end yet: `core/src/admin/**` (PostgreSQL databases, users, password reset; the SQL builder already has the MySQL/MariaDB dialect, the executor is PostgreSQL only), `core/src/update/**` (check, download, verify, plan, apply, no timer, no UI) and `core/src/clients/**` (detect, launch plan, injected spawner). `docs/architecture/admin.md` section "Proposed `DblService` surface" has the first sketch; `docs/architecture/update.md` and `clients.md` describe the other two. The GUI and the CLI may only use `DblService`.

## Deliverables

1. **Trait surface.** Decide the final shape in your first commit and update `docs/architecture/service.md`: database list/create/drop, user create, superuser password reset, update check/stage/apply, client detect and open. Long operations follow the existing pattern (`OperationId` plus `ServiceEvent`); fast reads return `Result`. New methods with provided default bodies are acceptable only if every implementor still must override them in practice; the mock implements all of them with deterministic data. New events if needed must be `#[non_exhaustive]`-safe for the GUI (list them in your report: W2-B/W4-B match on `ServiceEvent`).
2. **`LocalService` implementation** for PostgreSQL using `PgAdmin` against the running instance over loopback, credentials from the secret store via `CredentialSink`/`PasswordEngine`, never in events, logs, argv or error text (canary test). MySQL/MariaDB and Valkey return a typed, stable-coded, Indonesian "not supported for this engine yet" error until the MySQL/MariaDB executor task (W4-A2, after W3-B1/W3-B2 merge); the error must be tested per engine, not a `todo!()`.
3. **Update**: `check_update` only when the caller asks (no timer, no startup check unless the setting is on), result carries `InstallKind`; for package-manager installs the apply path is refused with the existing typed error and the message tells the user which package manager to use. Network goes through the existing fetcher traits so tests stay offline.
4. **Open in client**: `detect_clients` (cached for the process, refreshable) and `open_in_client(id, client, target)` building the plan with `plan_open`, password only through the environment/temp-file mechanisms the module already has; the service never returns the password.
5. **CLI** (`crates/cli/src/commands/`, new files, contract in `docs/architecture/cli.md`: stdout is data, stderr the rest, `--json`, exit codes): `dblocal db list|create|drop <instance> [name] [--yes --confirm <name>]`, `dblocal user create <instance> <name> --db <db> --level ro|rw`, `dblocal password reset <instance>` (prints the new URI once, to stdout, like `uri`), `dblocal update check|apply [--yes]`, `dblocal open <instance> --client <kind> [--dry-run]` (dry-run prints the plan without secrets). Drop requires the name to be typed again (PRD F-DBU-01 AC3).
6. **Real-engine e2e** through the public `DblService` on real PostgreSQL (extend `cargo xtask e2e`, serialized under the e2e lock): create database (UTF-8 check), list with sizes, drop with and without the confirmation, user read-only can SELECT and cannot INSERT, read-write can, password reset updates engine + keychain + URI, and the F-DBU-03 AC2 case (apply fails mid-way -> old password still works, keychain untouched).

## Mutation checks (paste red, then restored, `git status --short` empty)

Drop the typed-name confirmation; send the identifier unquoted; return the password in an event or in the error text; skip the rollback on a failed reset; let `update apply` run for a package-manager install.

## Bar

`cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings` (and `--features demo`), `cargo deny check`, new core lib tests x40 at load < 8, `cargo xtask ci` and `cargo xtask e2e` (locks as in rules 12/14), coverage: pure logic >= 90%, every touched file >= 80% (`cargo llvm-cov`, delete its target dir afterwards). Not verifiable here, list it: real GitHub release download/apply, Windows/macOS clients and terminals, MySQL/MariaDB admin.
