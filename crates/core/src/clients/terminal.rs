//! Terminal resolution and secure launcher-script preparation.
//!
//! CLI clients that need a password are launched through a private launcher
//! script on every OS (F1). The script exports env vars, deletes itself as its
//! first action, then execs the client.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use rand::TryRng;

use crate::secret::SecretString;

use super::detect::HostProbe;
use super::error::{ClientError, ClientResult};
use super::plan::{LaunchEnv, OsKind, TempFile, TerminalChoice, TerminalKind};
use super::quote::shell_quote;

pub const SCRIPT_PLACEHOLDER: &str = "{script}";
pub const SCRIPT_MAX_AGE: Duration = Duration::from_secs(10 * 60);

/// Effective user id of this process, used for the launcher-directory owner check.
///
/// `sysinfo` is already a dependency and its `user` feature is on by default; this
/// stays safe Rust (the workspace denies `unsafe_code`) and works on macOS, where
/// `/proc` does not exist.
pub fn current_uid() -> u32 {
    #[cfg(unix)]
    {
        use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
        let Ok(pid) = sysinfo::get_current_pid() else {
            return u32::MAX;
        };
        let Ok(mut system) = System::new() else {
            return u32::MAX;
        };
        system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[pid]),
            true,
            ProcessRefreshKind::nothing().with_user(UpdateKind::Always),
        );
        system
            .process(pid)
            .and_then(|process| process.effective_user_id().cloned())
            .map(|uid| *uid)
            .unwrap_or(u32::MAX)
    }
    #[cfg(not(unix))]
    {
        0
    }
}

/// Resolves the terminal to use for CLI clients on the given OS.
pub fn resolve_terminal(host: &dyn HostProbe, os: OsKind) -> ClientResult<TerminalChoice> {
    match os {
        OsKind::Linux => resolve_linux_terminal(host),
        OsKind::Macos => Ok(TerminalChoice::new("open", TerminalKind::MacOsTerminalApp)),
        OsKind::Windows => resolve_windows_terminal(host),
    }
}

fn resolve_linux_terminal(host: &dyn HostProbe) -> ClientResult<TerminalChoice> {
    if let Some(path) = std::env::var_os("TERMINAL") {
        let path = PathBuf::from(path);
        if !path.as_os_str().is_empty() && terminal_exists(host, &path) {
            return Ok(TerminalChoice::new(path, TerminalKind::GenericArgv));
        }
    }
    for (name, kind) in linux_terminal_candidates() {
        if let Some(path) = host.which(name) {
            return Ok(TerminalChoice::new(path, *kind));
        }
    }
    Err(ClientError::NoTerminal)
}

fn terminal_exists(host: &dyn HostProbe, path: &Path) -> bool {
    if path.components().count() > 1 {
        host.path_exists(path)
    } else {
        host.which(&path.to_string_lossy()).is_some()
    }
}

pub fn linux_terminal_candidates() -> &'static [(&'static str, TerminalKind)] {
    &[
        ("xdg-terminal-exec", TerminalKind::XdgTerminalExec),
        ("ptyxis", TerminalKind::Ptyxis),
        ("x-terminal-emulator", TerminalKind::GenericArgv),
        ("gnome-terminal", TerminalKind::GnomeTerminal),
        ("konsole", TerminalKind::Konsole),
        ("foot", TerminalKind::Foot),
        ("wezterm", TerminalKind::Wezterm),
        ("kitty", TerminalKind::Kitty),
        ("alacritty", TerminalKind::Alacritty),
        ("xterm", TerminalKind::Xterm),
    ]
}

fn resolve_windows_terminal(host: &dyn HostProbe) -> ClientResult<TerminalChoice> {
    if let Some(path) = host.which("wt.exe").or_else(|| host.which("wt")) {
        return Ok(TerminalChoice::new(path, TerminalKind::WindowsTerminal));
    }
    Ok(TerminalChoice::new("cmd.exe", TerminalKind::WindowsCmd))
}

/// Directory used for launcher scripts. One function for plan substitution and spawner.
pub fn launcher_dir(os: OsKind) -> ClientResult<PathBuf> {
    match os {
        OsKind::Linux => linux_launcher_dir(),
        OsKind::Macos => macos_launcher_dir(),
        OsKind::Windows => windows_launcher_dir(),
    }
}

fn linux_launcher_dir() -> ClientResult<PathBuf> {
    if let Some(xdg) = std::env::var_os("XDG_RUNTIME_DIR")
        && !xdg.is_empty()
    {
        return Ok(PathBuf::from(xdg).join("dblocal"));
    }
    let home = std::env::var_os("HOME").ok_or_else(|| ClientError::ScriptError {
        reason: "HOME missing".to_owned(),
    })?;
    Ok(PathBuf::from(home).join(".dblocal").join("run"))
}

fn macos_launcher_dir() -> ClientResult<PathBuf> {
    let tmp = std::env::var_os("TMPDIR").unwrap_or_else(|| OsString::from("/tmp"));
    Ok(PathBuf::from(tmp).join("dblocal"))
}

fn windows_launcher_dir() -> ClientResult<PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .ok_or_else(|| ClientError::ScriptError {
            reason: "LOCALAPPDATA missing".to_owned(),
        })?;
    Ok(PathBuf::from(base).join("dblocal").join("run"))
}

/// Builds a private launcher script temp file description.
pub fn launcher_temp_file(
    client_program: &Path,
    client_argv: &[String],
    env: &[LaunchEnv],
    os: OsKind,
) -> ClientResult<TempFile> {
    let name = random_script_name(os);
    let contents = match os {
        OsKind::Windows => {
            SecretString::new(build_windows_launcher(client_program, client_argv, env)?)
        }
        _ => SecretString::new(build_posix_launcher(client_program, client_argv, env)),
    };
    Ok(TempFile::new(name, contents, 0o600))
}

fn random_script_name(os: OsKind) -> String {
    let mut bytes = [0u8; 16];
    let _ = rand::rngs::SysRng.try_fill_bytes(&mut bytes);
    let mut hex = String::with_capacity(32);
    for b in bytes {
        use std::fmt::Write;
        let _ = write!(&mut hex, "{b:02x}");
    }
    let ext = if matches!(os, OsKind::Windows) {
        "cmd"
    } else {
        "sh"
    };
    format!("dblocal-open-{hex}.{ext}")
}

fn build_posix_launcher(
    client_program: &Path,
    client_argv: &[String],
    env: &[LaunchEnv],
) -> String {
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
) -> ClientResult<String> {
    for var in env {
        validate_windows_env_value(var.value().expose())?;
    }
    // cmd.exe re-reads .cmd between commands; keep delete/set/exec in one parsed block.
    let mut out = String::from("@echo off\r\n(\r\n  del \"%~f0\" >nul 2>&1\r\n");
    for var in env {
        out.push_str("  set \"");
        out.push_str(var.name());
        out.push('=');
        out.push_str(&escape_cmd_set_value(var.value().expose()));
        out.push_str("\"\r\n");
    }
    out.push_str("  ");
    out.push('"');
    out.push_str(&client_program.to_string_lossy().replace('"', "\"\""));
    out.push('"');
    for arg in client_argv {
        out.push(' ');
        out.push('"');
        out.push_str(&arg.replace('"', "\"\""));
        out.push('"');
    }
    out.push_str("\r\n)\r\n");
    Ok(out)
}

fn validate_windows_env_value(value: &str) -> ClientResult<()> {
    if value.contains('"') || value.contains('\r') || value.contains('\n') {
        return Err(ClientError::InvalidTarget { field: "password" });
    }
    Ok(())
}

fn escape_cmd_set_value(value: &str) -> String {
    // In cmd batch files, % introduces variable expansion; double it.
    value.replace('%', "%%")
}

/// Escapes a string for an AppleScript double-quoted literal.
pub fn escape_applescript(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Materializes a launcher script in the secure default directory.
pub fn materialize_launcher(temp: &TempFile, os: OsKind) -> ClientResult<PathBuf> {
    let dir = launcher_dir(os)?;
    materialize_launcher_in(&dir, temp)
}

/// Materializes a launcher script in the provided directory.
///
/// The directory is created and verified as secure before writing. Old
/// `dblocal-open-*` scripts in the directory are swept best-effort.
pub fn materialize_launcher_in(dir: &Path, temp: &TempFile) -> ClientResult<PathBuf> {
    materialize_launcher_in_with_uid(dir, temp, current_uid())
}

pub fn materialize_launcher_in_with_uid(
    dir: &Path,
    temp: &TempFile,
    uid: u32,
) -> ClientResult<PathBuf> {
    ensure_secure_dir_for(dir, uid)?;
    sweep_old_scripts(dir)?;
    write_script_file(dir, temp)
}

fn ensure_secure_dir_for(dir: &Path, uid: u32) -> ClientResult<()> {
    if dir.exists() {
        verify_secure_dir_for(dir, uid)?;
        return Ok(());
    }
    if let Some(parent) = dir.parent() {
        std::fs::create_dir_all(parent).map_err(|e| ClientError::ScriptError {
            reason: e.kind().to_string(),
        })?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(dir)
            .map_err(|e| ClientError::ScriptError {
                reason: e.kind().to_string(),
            })?;
        verify_secure_dir_for(dir, uid)?;
    }
    #[cfg(not(unix))]
    {
        let _ = uid;
        std::fs::create_dir_all(dir).map_err(|e| ClientError::ScriptError {
            reason: e.kind().to_string(),
        })?;
    }
    Ok(())
}

#[cfg(unix)]
pub fn verify_secure_dir_for(dir: &Path, uid: u32) -> ClientResult<()> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::symlink_metadata(dir).map_err(|e| ClientError::ScriptError {
        reason: e.kind().to_string(),
    })?;
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return Err(ClientError::ScriptError {
            reason: "not a real directory".to_owned(),
        });
    }
    if meta.uid() != uid {
        return Err(ClientError::ScriptError {
            reason: "wrong owner".to_owned(),
        });
    }
    if meta.mode() & 0o777 != 0o700 {
        return Err(ClientError::ScriptError {
            reason: "insecure mode".to_owned(),
        });
    }
    Ok(())
}

#[cfg(not(unix))]
pub fn verify_secure_dir_for(_dir: &Path, _uid: u32) -> ClientResult<()> {
    Ok(())
}

fn sweep_old_scripts(dir: &Path) -> ClientResult<()> {
    let now = SystemTime::now();
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return Ok(()),
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n,
            None => continue,
        };
        if !name.starts_with("dblocal-open-") {
            continue;
        }
        let meta = match std::fs::symlink_metadata(&path) {
            Ok(m) => m,
            Err(_) => continue,
        };
        if !meta.is_file() {
            continue;
        }
        let modified = match meta.modified() {
            Ok(m) => m,
            Err(_) => continue,
        };
        if now.duration_since(modified).unwrap_or(Duration::ZERO) > SCRIPT_MAX_AGE {
            let _ = std::fs::remove_file(&path);
        }
    }
    Ok(())
}

pub fn write_script_file(dir: &Path, temp: &TempFile) -> ClientResult<PathBuf> {
    let path = dir.join(temp.name_hint());
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(temp.mode())
            .open(&path)
            .map_err(|e| ClientError::ScriptError {
                reason: e.kind().to_string(),
            })?;
        file.write_all(temp.contents().expose().as_bytes())
            .map_err(|e| {
                let _ = std::fs::remove_file(&path);
                ClientError::ScriptError {
                    reason: e.kind().to_string(),
                }
            })?;
        file.sync_all().map_err(|e| {
            let _ = std::fs::remove_file(&path);
            ClientError::ScriptError {
                reason: e.kind().to_string(),
            }
        })?;
    }
    #[cfg(not(unix))]
    {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|e| ClientError::ScriptError {
                reason: e.kind().to_string(),
            })?;
        file.write_all(temp.contents().expose().as_bytes())
            .map_err(|e| {
                let _ = std::fs::remove_file(&path);
                ClientError::ScriptError {
                    reason: e.kind().to_string(),
                }
            })?;
        file.sync_all().map_err(|e| {
            let _ = std::fs::remove_file(&path);
            ClientError::ScriptError {
                reason: e.kind().to_string(),
            }
        })?;
    }
    Ok(path)
}

/// Substitute the script placeholder in an argv element.
pub fn substitute_script_placeholder(value: &str, script_path: &Path) -> String {
    value.replace(SCRIPT_PLACEHOLDER, &script_path.to_string_lossy())
}
