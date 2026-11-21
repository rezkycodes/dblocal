# W3-C fix round 3 — two Windows launcher mutations survive (value tests only)

Same branch `w3-open-client` (HEAD `b7fece6` reviewed). Rules: `docs/orchestration/tasks/_COMMON-RULES.md` (rules 11-13). Edit only `crates/core/src/clients/**` and `docs/architecture/clients.md`. No Rust production change is expected, only tests (and a doc line).

Round 2 verified by the reviewer on a trial merge with main: `cargo fmt`, `clippy -D warnings`, `cargo deny` green; `clients` tests 20 times `89 passed; 0 failed`; the FULL core lib 15 times `702 passed; 0 failed` (your two instance-test failures did not reproduce; they ran while other workers' e2e held ports 20000+, see rule 12); `cargo xtask ci` green on rerun; the real `ptyxis` test passes with and without a running ptyxis and leaves no `dblocal-open-*` file; the launcher directory is clean after all runs. Eleven of thirteen reviewer mutations are killed by a named test (password in argv, no `shell_quote`, fixed script name, `create_new` dropped, leading-dash identifiers, sweep off, directory mode, directory owner, any host, CLI plan ignoring the detected path, no self-delete) and the symlinked-directory mutation is an equivalent mutant. Do not rework any of that. Two survive, both Windows, both testable on Linux as plain string checks (round 1 F1 required them):

## W1 — `%` doubling in the generated `.cmd` is not tested

Mutation: `terminal.rs::escape_cmd_set_value` returning `value.to_owned()` (no `%%`): all tests green. Add a test that builds the Windows launcher for passwords `%PATH%`, `100%`, `a%%b`, `%` and asserts the exact `set "PGPASSWORD=..."` line (each `%` doubled, nothing else changed). Rerun the mutation: red.

## W2 — rejection of `"`, CR and LF in a Windows password is not tested

Mutation: `validate_windows_env_value` made a no-op (`if false && ...`): all green. Add tests: a password containing `"`, `\r`, `\n` makes `launcher_temp_file(.., OsKind::Windows)` (and `plan_open` for Windows) return the typed `InvalidTarget { field: "password" }` and writes nothing; the same corpus is accepted on Linux/macOS (the POSIX launcher quotes it, already covered). Rerun the mutation: red.

## While there

Pin the Windows launcher STRUCTURE with one golden test (value only, since cmd.exe cannot run here): the file starts with `@echo off\r\n(\r\n`, the `del "%~f0"` line comes before every `set` and before the client line, the client line is the last line inside the parentheses, the file ends with `)\r\n`, every line ends in `\r\n`, and no `set`/client line appears outside the block. Mutation: move `del` after the client line (or drop the opening parenthesis): red. State in `clients.md` that the Windows script and `cmd.exe` behaviour are unverified until the owner's Windows run (the checklist row is `docs/dev/windows-verification.md`: verify it names the "Open in..." launcher; if it does not, say so in your report and I will add the row).

## Bar

`cargo fmt --all -- --check`, `cargo clippy -p dblocal-core --all-targets -- -D warnings`, `cargo test -p dblocal-core --lib clients` x40, full lib once, `cargo deny check`, `cargo xtask ci`: pasted. The two mutations (and the structure mutation) red then restored, `git status --short` clean. `worker_done` only with pasted evidence.
