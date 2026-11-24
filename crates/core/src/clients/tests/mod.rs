mod applescript;
mod detect;
#[cfg(unix)]
mod f1_terminal;
mod golden;
#[cfg(unix)]
mod h4_launcher;
mod hostile;
mod plan;
mod plan_extra;
mod real_host;
mod resilient;
// These run real `#!/bin/sh` fake clients through `SystemSpawner` and wait for
// a shell redirection to land. Windows has no `/bin/sh`, and a `#!` script is
// not executable there, so they are POSIX-only; the Windows launcher is proved
// by `windows_script`.
#[cfg(unix)]
mod run_extra;
mod scan;
#[cfg(unix)]
mod spawn;
mod terminal_extra;
mod windows_script;
