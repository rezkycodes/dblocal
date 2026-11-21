# Open in external client (W3-C)

This document records the detection and launch planning behind F-OPN-01 and
F-OPN-02. Code: `crates/core/src/clients/`. The module is core-only; the GUI
wires the menu later.

## Scope

- `detect_clients(&dyn HostProbe) -> Vec<DetectedClient>`: pure over an injected
  host probe.
- `plan_open(client, &OpenTarget, Option<&SecretString>) -> Result<LaunchPlan>`:
  pure value describing how to open a client.
- `run(&LaunchPlan, &dyn Spawner) -> Result<()>`: executes the plan through an
  injected spawner.

The password never appears in argv, in a URL printed in a command line, in a log
line, or in an error message. It is passed only through the spawned process
environment (`PGPASSWORD`, `MYSQL_PWD`, `REDISCLI_AUTH`), or through a private
launcher script when a terminal cannot inherit the environment safely.

## Host probe

`HostProbe` abstracts OS lookups so tests inject a fake host:

- `which(name) -> Option<PathBuf>`: PATH lookup.
- `path_exists(path) -> bool`.
- `desktop_entries() -> Vec<String>`: Linux `.desktop` ids / names.
- `macos_applications() -> Vec<String>`: app bundle names under `/Applications`.
- `windows_app_paths() -> Vec<String>`: App Paths registry display names/paths.

The real probe is not implemented here (core-only, no GUI/OS integration in this
task); tests use `FakeHost`.

## Known clients and engine matrix

Only clients that are really found are returned. A client declares which engines
it can open; unsupported combinations are rejected by `plan_open`.

| Client | Kind | PostgreSQL | MySQL | MariaDB | Valkey | Evidence / note |
|---|---|:---:|:---:|:---:|:---:|---|
| DBDelve | GUI | yes | yes | yes | no | DBDelve is a PostgreSQL/MySQL-family desktop client; Valkey/Redis not claimed without source. |
| TablePlus | GUI | yes | yes | yes | yes | TablePlus documents native connections for PostgreSQL, MySQL, MariaDB and Redis-compatible servers; `tableplus://` URL scheme is documented. |
| DBeaver | GUI | yes | yes | yes | yes | DBeaver supports PostgreSQL, MySQL/MariaDB and Redis via drivers; opened via `-con` command-line argument with a URL-like connection spec. Valkey is RESP-compatible; support depends on driver, so we only enable when detected and treat as Redis-compatible. |
| `psql` | CLI | yes | no | no | no | PostgreSQL interactive terminal. |
| `mysql` | CLI | no | yes | yes | no | MySQL/MariaDB CLI. |
| `mariadb` | CLI | no | yes | yes | no | MariaDB CLI, compatible with MySQL protocol. |
| `valkey-cli` | CLI | no | no | no | yes | Valkey CLI; reads `REDISCLI_AUTH` for password. |
| `redis-cli` | CLI | no | no | no | yes | Redis CLI; reads `REDISCLI_AUTH`. |

Uncertain support is intentionally excluded. DBDelve+Valkey is not advertised,
so it is left out.

## Launch planning

`LaunchPlan` is a pure value: `program`, `argv`, `env`, optional `url`, optional
`temp_file`. `plan_open` validates that the client supports the target engine and
that a detected client has not disappeared.

### GUI clients

- **TablePlus**: uses the documented `tableplus://` URL scheme. The password is
  **not** embedded in the URL. TablePlus prompts for the password or uses its own
  saved connection; we pass host/port/user/database/driver in the URL and omit
  password. Residual exposure: none in process args; the user is prompted by
  TablePlus if it has no saved credential.
- **DBeaver**: uses `-con` with a connection spec. The password is not placed in
  argv. DBeaver can prompt or use its secure storage. We pass a connection name
  and driver/url properties without password. Residual exposure: none in argv.
- **DBDelve**: opened with a connection URI argument when supported, but password
  is omitted from argv/URL; DBDelve prompts. If DBDelve requires password in URL,
  that would be a leak - we avoid it by omitting password and documenting the
  prompt behaviour. Residual exposure: user prompt.

### CLI clients

CLI clients run inside an OS terminal emulator. Passwords are passed by
environment variables on the spawned client process:

| Engine | Env var | Notes |
|---|---|---|
| PostgreSQL | `PGPASSWORD` | Read by `psql`. |
| MySQL/MariaDB | `MYSQL_PWD` | Read by `mysql`/`mariadb` client. Deprecated upstream but still supported; no argv alternative is safe. |
| Valkey | `REDISCLI_AUTH` | Read by `valkey-cli` and `redis-cli`. |

`PGPASSWORD`/`MYSQL_PWD`/`REDISCLI_AUTH` are set in the environment of the
spawned terminal/client process, not printed in command strings.

## Terminal handling

Terminal choice is per OS, exposed as `OsKind` for deterministic tests:

- **Linux**: `$TERMINAL` if set and executable, else `x-terminal-emulator`,
  `gnome-terminal`, `konsole`, `kitty`, `alacritty`, `xterm`.
- **macOS**: `Terminal.app` via `open -a Terminal` / `osascript`.
- **Windows**: `wt.exe`, falling back to `cmd.exe /k`.

When a terminal cannot inherit environment variables safely through its command
string (macOS `Terminal.app` via `osascript`, Windows `wt.exe`/`cmd.exe` command
line), the password must not be placed in the command string. The plan uses a
private launcher script in a `0700` directory: the script exports the env var,
removes itself as its first action, then execs the client. The script contains
the password; it lives in a private temp directory with mode `0700` and the file
mode `0600`, and is removed by the script itself before the client runs.
Residual exposure: the password exists briefly in a private file; on multi-user
systems with a compromised same-user process it could be read during that window.
This is safer than argv or command-line exposure in the process list.

Linux terminals that accept `-e`/`--` argv can receive env vars through the
spawned process environment, so no launcher script is needed.

## Injection and quoting

Instance names, hosts, URIs and paths are untrusted input. The module never
builds a shell string by concatenation. Where a shell is unavoidable (a
terminal's `-e` argument that takes a single command string, or a launcher
script body), `shell_quote` is used. It wraps values in single quotes and escapes
embedded single quotes as `'\''`. A leading `-` is safe when passed as argv; when
placed after `--` or as a quoted argument it is not interpreted as an option.

Hostile corpus tested: spaces, quotes, `;`, `&&`, `$()`, backticks, newlines,
`%`, leading `-`, very long names, non-ASCII.

## Secrets scanning

Tests include an assertion helper that scans every `LaunchPlan` for the secret in
argv, URL, and command string fields. It is used on every golden plan. A mutation
that puts the password into argv or URL is caught by this helper.

## Unverified / limitations

- Windows and macOS terminal launching is tested by golden plan value only; no
  real Windows/macOS terminal was executed on this Linux host.
- Real third-party client behaviour with the chosen mechanism was not tried
  unless the client is installed on this host. DBDelve, TablePlus, DBeaver were
  not exercised as real GUI launches here; detection on this host is printed by
  tests but not asserted as present.
- `MYSQL_PWD` is deprecated by MySQL upstream but remains the safest non-interactive
  option without writing a config file; we document the residual deprecation.
- macOS `osascript`/Terminal.app launcher script is the safest option found that
  avoids command-line password exposure; it has a brief private-file window.
- DBeaver `-con` argument format varies by version; we use a conservative generic
  connection argument and document that real-client verification is needed.
