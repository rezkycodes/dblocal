# W4-D — autostart at login and autostart of chosen instances (F-AUT-01, F-AUT-02), core and CLI first

Start after W2-B is merged (the GUI tray/quit/bootstrap files are W2-B's until then). New branch from `main`. You own a new `crates/core/src/autostart/**`, `docs/architecture/autostart.md`, the `autostart` CLI command file, and exactly the places in `crates/gui/src/app/bootstrap.rs` and `crates/cli` that start the app in "login item" mode; coordinate the rest through `ask`. Rules: `docs/orchestration/tasks/_COMMON-RULES.md` incl. 5, 11-14.

## Situation

`DblService::set_autostart(id, bool)` already stores the per-instance flag. Nothing reads it, nothing registers a login item, and the app has no "started by the login item" mode.

## Deliverables

1. **Login item per OS behind a trait** (`AutostartBackend`, one implementation per OS selected with `cfg`, all of them testable with an injected filesystem root and an injected command runner): Linux `systemd --user` unit file under `$XDG_CONFIG_HOME/systemd/user/` (PRD F-AUT-01 AC1), enabled with `systemctl --user`, falling back to an XDG autostart `.desktop` file when there is no user systemd; macOS LaunchAgent plist in `~/Library/LaunchAgents/`; Windows `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`. No sudo, nothing outside the user's config, nothing written when the setting is off, removal deletes every file or value it created (AC2) and is idempotent. The command line is the absolute path of the running binary, quoted per OS (paths with spaces, `Application Support`, `Program Files`), plus `--autostart`.
2. **`--autostart` mode**: opens straight to the tray with no window (AC3); if the tray is unavailable it must still not show a surprise window: log and keep running headless only if instances were started, otherwise exit with a clear message (decide, document, test the decision function purely).
3. **Instance autostart (F-AUT-02)**: on `--autostart`, start every instance with the flag, independently and concurrently bounded; one failure never blocks the others and never opens a modal dialog (AC2); failures become `failed` status visible in tray and window via the existing events. Pure decision function (which instances, in what order, what to do with a port conflict) tested without engines; one real PostgreSQL e2e (e2e lock, ports 20000+): two flagged instances, one with its port taken by a foreign listener -> the other starts, the failed one shows the diagnosis.
4. **CLI**: `dblocal autostart status|enable|disable` (`--json`, exit codes per `docs/architecture/cli.md`).

## Rules specific to this task

You must NOT run the real `systemctl --user enable`, write a real LaunchAgent/Registry value, or touch the user's real `~/.config` during development: tests use a temp root and a fake command runner. The reviewer verifies the generated unit with `systemd-analyze verify` on a copy and does one real enable/disable round trip himself. Windows and macOS code is compile-checked (`--target x86_64-pc-windows-gnu`; macOS cannot be compiled here, say so) and listed in `docs/dev/windows-verification.md` as rows for the user to check after the reboot (value present/removed, app starts to tray, quoted path with spaces).

## Mutation checks

Leave the files behind after disable; skip quoting of a path with spaces; start instances serially so one hanging start blocks the rest; show a dialog on failure: each must turn a test red (paste, then restored, `git status --short` empty).

## Bar

fmt, clippy workspace (and `--features demo`), `cargo deny check`, new tests x40 at load < 8, `cargo xtask ci` and the e2e under their locks, pure logic >= 90% and every touched file >= 80% coverage.
