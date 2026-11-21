//! Terminal resolution and launcher-script generation.
//!
//! CLI clients run in an OS terminal. When the terminal cannot safely inherit
//! environment variables through its command line, we generate a private launcher
//! script that exports env vars, deletes itself, then execs the client.

use std::path::{Path, PathBuf};

use crate::secret::SecretString;

use super::detect::HostProbe;
use super::plan::{LaunchEnv, OsKind, TempFile, TerminalChoice, TerminalKind};
use super::quote::shell_quote;

/// Resolves the terminal to use for CLI clients on the given OS.
pub fn resolve_terminal(host: &dyn HostProbe, os: OsKind) -> TerminalChoice {
    match os {
        OsKind::Linux => resolve_linux_terminal(host),
        OsKind::Macos => TerminalChoice::new("open", TerminalKind::MacOsTerminalApp),
        OsKind::Windows => resolve_windows_terminal(host),
    }
}

fn resolve_linux_terminal(host: &dyn HostProbe) -> TerminalChoice {
    if let Some(path) = std::env::var_os("TERMINAL") {
        let path = PathBuf::from(path);
        if !path.as_os_str().is_empty() {
            return TerminalChoice::new(path, TerminalKind::GenericArgv);
        }
    }
    for (name, kind) in [
        ("x-terminal-emulator", TerminalKind::GenericArgv),
        ("gnome-terminal", TerminalKind::GnomeTerminal),
        ("konsole", TerminalKind::Konsole),
        ("kitty", TerminalKind::Kitty),
        ("alacritty", TerminalKind::Alacritty),
        ("xterm", TerminalKind::Xterm),
    ] {
        if let Some(path) = host.which(name) {
            return TerminalChoice::new(path, kind);
        }
    }
    TerminalChoice::new("xterm", TerminalKind::Xterm)
}

fn resolve_windows_terminal(host: &dyn HostProbe) -> TerminalChoice {
    if let Some(path) = host.which("wt.exe").or_else(|| host.which("wt")) {
        return TerminalChoice::new(path, TerminalKind::WindowsTerminal);
    }
    TerminalChoice::new("cmd.exe", TerminalKind::WindowsCmd)
}

/// Builds a private launcher script temp file for terminals that cannot safely
/// inherit env vars via command string.
pub fn launcher_temp_file(
    client_program: &Path,
    client_argv: &[String],
    env: &[LaunchEnv],
    os: OsKind,
) -> TempFile {
    match os {
        OsKind::Windows => TempFile::new(
            "dblocal-open-client.cmd",
            SecretString::new(build_windows_launcher(client_program, client_argv, env)),
            0o600,
        ),
        _ => TempFile::new(
            "dblocal-open-client.sh",
            SecretString::new(build_posix_launcher(client_program, client_argv, env)),
            0o600,
        ),
    }
}

/// Path hint used in macOS open command.
pub fn macos_launcher_script_path(name: &str) -> String {
    format!("/tmp/dblocal-open-client/{name}")
}

/// Path hint used in Windows command.
pub fn windows_launcher_script_path(name: &str) -> String {
    format!("%TEMP%\\dblocal-open-client\\{name}")
}

fn build_posix_launcher(client_program: &Path, client_argv: &[String], env: &[LaunchEnv]) -> String {
    let mut out = String::from("#!/bin/sh\nset -eu\nrm -f -- \"$0\"\n");
    for var in env {
        out.push_str("export ");
        out.push_str(var.name());
        out.push('=');
        out.push_str(&shell_quote(var.value().expose()));
        out.push('\n');
    }
    out.push_str("exec ");
    out.push_str(&shell_quote(&client_program.to_string_lossy()));
    for arg in client_argv {
        out.push(' ');
        out.push_str(&shell_quote(arg));
    }
    out.push('\n');
    out
}

fn build_windows_launcher(
    client_program: &Path,
    client_argv: &[String],
    env: &[LaunchEnv],
) -> String {
    let mut out = String::from("@echo off\r\ndel \"%~f0\" >nul 2>&1\r\n");
    for var in env {
        out.push_str("set \"");
        out.push_str(var.name());
        out.push('=');
        out.push_str(&escape_cmd_set_value(var.value().expose()));
        out.push_str("\"\r\n");
    }
    out.push('"');
    out.push_str(&client_program.to_string_lossy().replace('"', "\"\""));
    out.push('"');
    for arg in client_argv {
        out.push(' ');
        out.push('"');
        out.push_str(&arg.replace('"', "\"\""));
        out.push('"');
    }
    out.push_str("\r\n");
    out
}

fn escape_cmd_set_value(value: &str) -> String {
    value.replace('\r', "").replace('\n', "").replace('"', "\"\"")
}

/// Escapes a string for an AppleScript double-quoted literal.
pub fn escape_applescript(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}
