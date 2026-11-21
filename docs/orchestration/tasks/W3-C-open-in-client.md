# W3-C — Open in an external client (F-OPN-01, F-OPN-02), core only

Branch `w3-open-client` from `main`. Read `docs/orchestration/tasks/_COMMON-RULES.md` first. PRD: **F-OPN-01..02**, `F-CRD-01..03`, `docs/architecture/secrets.md`, `docs/architecture/engines.md` (the URI builder `engine::uri`), D-18. Core-only: `CARGO_TARGET_DIR=$HOME/.cache/dblocal-target`.

## Goal

The logic behind the "Open in..." menu, without UI: which clients are installed on this machine, and how to open one on an instance's connection so that **the password never appears in a process argument list**. The GUI wires it later.

## You own

New module `crates/core/src/clients/**` with its tests, `docs/architecture/clients.md`, one `pub mod clients;` line in `crates/core/src/lib.rs`. You may not edit other core modules, `crates/gui/**` or `crates/cli/**`.

## Requirements

1. **Detect** (AC1/AC2): `detect_clients(host: &dyn HostProbe) -> Vec<DetectedClient>` where `HostProbe` is a small trait (PATH lookup, file exists, list `.desktop` entries / macOS `/Applications` / Windows `App Paths` registry reads) so tests inject a fake host. Known clients: DBDelve, TablePlus, DBeaver (GUI, any OS where they exist), and the CLIs `psql`, `mysql`/`mariadb`, `valkey-cli`, `redis-cli`. Only clients that are really found are returned, each with the proof (path). Each client declares which engines it can open (PostgreSQL: psql, DBeaver, TablePlus, DBDelve; MySQL family: mysql/mariadb CLI, DBeaver, TablePlus, DBDelve; Valkey: valkey-cli, redis-cli, TablePlus, DBeaver where it supports it; write the matrix in the doc with the source of each claim, and when you are not sure a client supports an engine, leave it out). The result for the menu is `OpenMenu { clients, copy_uri_fallback: bool }`; with nothing detected it contains only the "Salin URI" fallback, never an empty menu.
2. **Plan, then run**: `plan_open(client, instance: &OpenTarget, credential: Option<&SecretString>) -> Result<LaunchPlan>` is a PURE value: program, argv, environment, optional URL, optional temp-file. `run(plan, spawner: &dyn Spawner)` executes it through an injected spawner. GUI clients: use the URL scheme where one exists and is documented (`tableplus://`, DBeaver's `-con` argument), otherwise arguments; CLI clients: open the OS terminal emulator running the client. Terminal choice per OS: Linux (`x-terminal-emulator`, `gnome-terminal`, `konsole`, `kitty`, `alacritty`, `xterm` in a documented order; `$TERMINAL` if set), macOS (Terminal.app), Windows (`wt.exe`, falling back to `cmd.exe /k`).
3. **Secrets (AC2)**: the password is passed by environment variable on the spawned process (`PGPASSWORD`, `MYSQL_PWD`, `REDISCLI_AUTH`, check what `valkey-cli` reads) and NEVER in argv, a URL printed in a command line, a log line or an error message. Where an environment variable cannot reach the final shell (macOS `Terminal.app` via `osascript`, Windows `wt.exe`), do not put the password in the command string either: analyse the options (a launcher script in a 0700 private directory that exports the variable, removes itself first thing and is documented, or a connection URL with no password plus a prompt) and implement the safest one that works, explaining the residual exposure in the doc. A GUI client given a `tableplus://` URL that embeds the password is a leak in the process list: prefer the client's own prompt or saved-connection mechanism and say what you chose and why.
4. **Injection**: instance names, hosts and URIs are untrusted input. Never build a shell string by concatenation; use argv spawning. Where a shell is unavoidable (a terminal's `-e`), quote with one tested function. Test with a hostile corpus: spaces, quotes, `;`, `&&`, `$()`, backticks, newlines, `%`, a leading `-` (option injection), very long names, non-ASCII.
5. A client that disappears between detection and launch returns a typed error, not a panic.

## Tests and bar

Golden `LaunchPlan` values per client/engine/OS (the OS is a parameter, so Windows and macOS plans are tested on Linux by value); an assertion helper that scans every plan for the secret in argv/URL/command string, used on every golden; the hostile corpus; real detection on this Linux host printed (not asserted). Mutation checks: put the password into argv in the plan builder and show the secret-scan test failing; remove the quoting function and show the corpus failing. Real spawn tested only with a harmless fake program in a temp dir. The usual bar from the common rules.

## Done when

Everything above plus the unverified list: Windows/macOS terminal launching, the real behaviour of each third-party client with the chosen mechanism (DBDelve, TablePlus, DBeaver are not installed here unless you find them: say which you could not try).
