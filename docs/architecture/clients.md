# Open in external client (W3-C)

This document records the detection and launch planning behind F-OPN-01 and
F-OPN-02. Code: `crates/core/src/clients/`. The module is core-only; the GUI
wires the menu later.

## Scope

- `detect_clients(&dyn HostProbe) -> Vec<DetectedClient>`: pure over an injected
  host probe.
- `plan_open(client, &OpenTarget, Option<&SecretString>, OsKind, &TerminalChoice) -> ClientResult<LaunchPlan>`:
  pure value describing how to open a client. `OpenTarget::new` validates untrusted input.
- `run(&LaunchPlan, &dyn Spawner) -> ClientResult<()>`: executes the plan through an
  injected spawner. The spawner materializes secure launcher scripts and substitutes `{script}`.

The password never appears in argv, in a URL printed in a command line, in a log
line, or in an error message. CLI passwords are passed only through private launcher
scripts that export env vars and self-delete.

## Host probe

`HostProbe` abstracts OS lookups so tests inject a fake host: `which`, `path_exists`,
`desktop_entries`, `macos_applications`, `windows_app_paths`. Real probes are outside
this core-only task.

## Known clients and engine matrix

| Client | Kind | PostgreSQL | MySQL | MariaDB | Valkey | Evidence / note |
|---|---|:---:|:---:|:---:|:---:|---|
| DBDelve | GUI | yes | yes | yes | no | DBDelve connection-argument mechanism is **unverified** on this machine. Plan launches the detected DBDelve app only and sets `copy_uri_hint` with a password-less URI so GUI can copy it. `dbdelve --help` produced no output in this headless environment. |
| TablePlus | GUI | yes | yes | yes | yes | Official TablePlus docs document opening a connection by URL form like `postgresql://user@host:port/db` with values URL-encoded and no password in URL: https://docs.tableplus.com/gui-tools/manage-connections . macOS uses `open -a TablePlus <url>` (documented). Linux/Windows pass URL to detected TablePlus executable path. `mysql://` and `redis://` URL forms for TablePlus are **unverified** here (third-party reports only). |
| DBeaver | GUI | yes | yes | yes | yes | DBeaver CLI `-con` is documented: https://dbeaver.com/docs/dbeaver/Command-Line/ . `driver`, `host`, `port`, `name`, `openConsole` are documented parameters; MariaDB driver id `maria` is **unverified**. Password is never passed in argv. |
| `psql` | CLI | yes | no | no | no | PostgreSQL CLI. Uses `postgresql://user@host:port/db` conninfo arg to avoid libpq keyword conninfo injection; password via `PGPASSWORD` in launcher script. |
| `mysql` | CLI | no | yes | yes | no | MySQL/MariaDB CLI. Uses `--database=<value>` to avoid positional option injection; password via `MYSQL_PWD` in launcher script. |
| `mariadb` | CLI | no | yes | yes | no | MariaDB CLI. Same safe argv form as mysql. |
| `valkey-cli` | CLI | no | no | no | yes | Valkey CLI. Password via `REDISCLI_AUTH` in launcher script. |
| `redis-cli` | CLI | no | no | no | yes | Redis CLI. Password via `REDISCLI_AUTH` in launcher script. |

## OpenTarget validation (F2)

`OpenTarget::new` returns `ClientResult` and rejects unsafe input:

- host: only `127.0.0.1`, `::1`, `localhost`.
- username/database: `[A-Za-z0-9_.-]`, max 63 chars, not starting with `-`; database may be empty.
- instance name: no control characters, max 128 chars.
- port: 1..=65535.

CLI args never pass database/user as positional/conninfo-injectable values: mysql/mariadb use `--database=<value>`, psql uses URI conninfo `postgresql://user@host:port/db` with password omitted.

## Launch planning

`LaunchPlan` is pure data: `program`, `argv`, `env`, optional `url`, optional `temp_file`,
optional `command_string`, `required_path`, `copy_uri_hint`. Secrets never go in argv/url/command_string.
CLI plans with passwords use a launcher script and `{script}` placeholder substituted by the spawner.

## CLI secrets and launcher scripts (F1)

All OSes use the same launcher-script mechanism for CLI clients that need a password. This avoids
single-instance terminal env inheritance failure (verified on this machine with ptyxis: a second
window did not inherit caller env var).

Launcher directory (one function): Linux `$XDG_RUNTIME_DIR/dblocal` or fallback `$HOME/.dblocal/run`;
macOS `$TMPDIR/dblocal` (or `/tmp/dblocal`); Windows `%LOCALAPPDATA%\dblocal\run` or `%USERPROFILE%\dblocal\run`.
Directory is created 0700; existing dir is verified via `symlink_metadata`: real dir (a symlink is
refused), owner equal to the process's effective uid, mode exactly 0700. `verify_secure_dir_for(dir, uid)`
takes the expected uid as a parameter, so the owner check is testable with a wrong uid; the effective
uid comes from `sysinfo` (`effective_user_id`, safe Rust, works on macOS where `/proc` does not exist).
If verification fails, return typed `ScriptError` and never chmod.

Script file: random 128-bit name, `O_CREAT|O_EXCL` (a second write to the same name fails), mode 0600,
content written, `sync_all`.

### Exact guarantee

The launcher script exists on disk only between creation and the terminal starting it. While it exists
it is owner-only (`0600`) inside an owner-only `0700` directory and has a random 128-bit name; it is
deleted by its own first command (`rm -f -- "$0"` on POSIX; cmd block with `del "%~f0"` on Windows) and,
if the terminal never starts it, swept on the next launch once it is older than 10 minutes.
The spawner also deletes the script if the spawn itself fails. Secrets live only in that file; they are
never in argv, URL, command string, or logs.

### Verified command forms on this machine

- **ptyxis** (Ptyxis 50.1): `ptyxis --new-window -- sh {script}` opens a new window that runs the script.
  Verified by real execution in the `#[ignore]`d e2e test `real_ptyxis_receives_password_via_script`
  with a ptyxis instance already running (the single-instance case that env inheritance would break):
  the fake client observed `PGPASSWORD` and no `dblocal-open-*` file remained afterwards.
- All other terminal forms (gnome-terminal, konsole, foot, wezterm, kitty, xterm, xdg-terminal-exec) are
  from documentation and covered only by value-level tests, not by real execution here.

## Terminal handling (Linux)

Order: `$TERMINAL` if set and existing, then `xdg-terminal-exec`, `ptyxis`, `x-terminal-emulator`,
`gnome-terminal`, `konsole`, `foot`, `wezterm`, `kitty`, `alacritty`, `xterm`. If none found,
return `NoTerminal`.

Verified on this machine: `ptyxis --help` shows `--` command form and `--new-window`; no `-e`.
`xdg-terminal-exec`, `foot`, `wezterm`, `kitty` not installed here; their flags are from documentation
and marked unverified. Kitty uses positional command after `--` (unverified locally). macOS Terminal.app
and Windows `wt.exe`/`cmd.exe` paths are value-tested only.

## CLI client path

CLI plans exec the **detected absolute client path** (`client.path()`), never a bare `psql`/`mysql`/...
name. The GUI process's `PATH` is not the terminal shell's `PATH`; only the detected path guarantees the
client that was found is the client that runs. The plan also carries `required_path`, so a client that
disappeared between detection and launch fails with a typed `ClientNotFound` instead of a bare spawn error.


## Injection and quoting

No shell command is built by concatenation. Launcher scripts use `shell_quote` for POSIX and quoted args
for Windows cmd. Hostile corpus tested at validation and plan layers.

## Secrets scanning

Tests scan every `LaunchPlan` for secret in argv/url/command_string. Env values may contain secrets by design;
they are not used for CLI terminal launch plans after F1 (secrets live in scripts).

## Unverified / limitations

- Windows script behaviour, macOS Terminal.app, and terminals not installed locally (xdg-terminal-exec, foot, wezterm, kitty, alacritty, konsole, gnome-terminal, xterm) are unverified by real execution here.
- Real DBDelve/TablePlus/DBeaver behavior is unverified; DBDelve on host was detected but not launched.
- TablePlus mysql/redis URL forms are unverified by official docs (third-party reports only).
- DBeaver MariaDB driver id `maria` is unverified.
- `MYSQL_PWD` is deprecated upstream but used for non-interactive CLI compatibility.
