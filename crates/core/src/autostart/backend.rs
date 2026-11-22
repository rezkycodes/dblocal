//! Login-item backends for Linux, macOS and Windows (F-AUT-01).
//!
//! Every backend implements [`AutostartBackend`]. Production callers build the
//! platform backend through [`backend_for`]; tests inject a fake command runner
//! and a temp config root so nothing on the real machine is touched.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::platform::Os;

use super::consts;
use super::error::{AutostartError, AutostartResult};

/// Result of enabling or disabling the login item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginItemStatus {
    /// Whether the login item exists after the operation.
    pub enabled: bool,
    /// Absolute path of the file or registry value that represents the item.
    pub location: PathBuf,
}

/// What the login item should run.
///
/// `exe` is the absolute path of the running binary; the backend adds
/// [`consts::AUTOSTART_FLAG`] and quotes the path in the form its OS accepts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginItem {
    /// Absolute path of the executable to run at login.
    pub exe: PathBuf,
    /// Extra arguments appended after the executable, in order.
    pub args: Vec<String>,
}

impl LoginItem {
    /// The standard login item: this binary plus `--autostart`.
    pub fn for_exe(exe: impl Into<PathBuf>) -> AutostartResult<Self> {
        let exe = exe.into();
        if !exe.is_absolute() {
            return Err(AutostartError::InvalidExe {
                reason: "not absolute",
            });
        }
        Ok(Self {
            exe,
            args: vec![consts::AUTOSTART_FLAG.to_owned()],
        })
    }

    /// The full command as individual argv tokens.
    pub fn argv(&self) -> Vec<String> {
        let mut out = Vec::with_capacity(1 + self.args.len());
        out.push(self.exe.display().to_string());
        out.extend(self.args.iter().cloned());
        out
    }
}

/// Output of a helper command, captured for tests and diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutput {
    /// Process exit code, or `-1` when the process could not be started.
    pub code: i32,
    /// Captured standard output, trimmed to a small diagnostic.
    pub stdout: String,
    /// Captured standard error, trimmed to a small diagnostic.
    pub stderr: String,
}

impl CommandOutput {
    /// A successful exit.
    pub fn ok() -> Self {
        Self {
            code: 0,
            stdout: String::new(),
            stderr: String::new(),
        }
    }

    /// Whether the command exited with code 0.
    pub fn success(&self) -> bool {
        self.code == 0
    }
}

/// Runs a helper command (`systemctl --user`, `launchctl`, `reg`).
///
/// Production backends use [`SystemRunner`]; tests inject a fake that records
/// calls and returns canned outputs without touching the host.
pub trait CommandRunner: Send + Sync {
    /// Run `program` with `args` and return the captured output.
    fn run(&self, program: &str, args: &[String]) -> AutostartResult<CommandOutput>;
}

/// Production runner: spawns the command and captures its output.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemRunner;

impl CommandRunner for SystemRunner {
    fn run(&self, program: &str, args: &[String]) -> AutostartResult<CommandOutput> {
        let output = std::process::Command::new(program)
            .args(args)
            .output()
            .map_err(|err| AutostartError::CommandFailed {
                program: program_to_static(program),
                detail: err.to_string(),
            })?;
        let code = output.status.code().unwrap_or(-1);
        Ok(CommandOutput {
            code,
            stdout: truncate_diag(&String::from_utf8_lossy(&output.stdout)),
            stderr: truncate_diag(&String::from_utf8_lossy(&output.stderr)),
        })
    }
}

fn truncate_diag(text: &str) -> String {
    let trimmed = text.trim();
    const MAX: usize = 240;
    if trimmed.len() <= MAX {
        trimmed.to_owned()
    } else {
        trimmed.chars().take(MAX).collect()
    }
}

fn program_to_static(program: &str) -> &'static str {
    match program {
        "systemctl" => "systemctl",
        "launchctl" => "launchctl",
        "reg" => "reg",
        _ => "command",
    }
}

/// One platform's way to register or remove a per-user login item.
pub trait AutostartBackend: Send + Sync {
    /// Write (or refresh) the login item so `item` runs at user login.
    fn enable(&self, item: &LoginItem) -> AutostartResult<LoginItemStatus>;
    /// Remove every file or value this backend may have created.
    ///
    /// Must be idempotent: removing an already-removed item is a success.
    fn disable(&self) -> AutostartResult<LoginItemStatus>;
    /// Report the current login-item state without changing it.
    fn status(&self) -> AutostartResult<LoginItemStatus>;
}

/// Select the production backend for the current OS.
///
/// `home` is the user's home directory (`$HOME` / `%USERPROFILE%`).
/// `config_home` is `$XDG_CONFIG_HOME` or `home/.config` on Linux; on other
/// platforms it may be the same as `home` and is ignored by the backend.
pub fn backend_for(
    os: Os,
    home: &Path,
    config_home: &Path,
    runner: Arc<dyn CommandRunner>,
) -> Box<dyn AutostartBackend> {
    match os {
        Os::Linux => Box::new(super::linux::LinuxBackend::new(home, config_home, runner)),
        Os::Macos => Box::new(super::macos::MacosBackend::new(home, runner)),
        Os::Windows => Box::new(super::windows::WindowsBackend::new(home, runner)),
    }
}

/// Quote a path for a POSIX shell command line inside a systemd `ExecStart=`.
///
/// systemd uses a shell-like quoting for `ExecStart`, but the simplest form
/// that survives spaces is a double-quoted absolute path with backslash
/// escapes for `"`, `\`, `$` and `` ` ``. We reject newlines and NUL because
/// a unit file is line-oriented and a NUL is never a valid path component.
pub fn quote_systemd_exec(path: &Path) -> AutostartResult<String> {
    let raw = path.to_str().ok_or(AutostartError::InvalidExe {
        reason: "not utf-8",
    })?;
    if raw.is_empty() {
        return Err(AutostartError::InvalidExe { reason: "empty" });
    }
    if raw.chars().any(|c| c == '\n' || c == '\r' || c == '\0') {
        return Err(AutostartError::InvalidExe {
            reason: "contains control character",
        });
    }
    let mut quoted = String::with_capacity(raw.len() + 2);
    quoted.push('"');
    for ch in raw.chars() {
        match ch {
            '\\' | '"' | '$' | '`' => {
                quoted.push('\\');
                quoted.push(ch);
            }
            _ => quoted.push(ch),
        }
    }
    quoted.push('"');
    Ok(quoted)
}

/// Quote an argv token for a POSIX shell (used by the XDG desktop `Exec=`).
///
/// Single quotes are the safest form: every character is literal except a
/// single quote itself, which we spell as `'\''`.
pub fn quote_posix_single(raw: &str) -> AutostartResult<String> {
    if raw.chars().any(|c| c == '\n' || c == '\r' || c == '\0') {
        return Err(AutostartError::InvalidExe {
            reason: "contains control character",
        });
    }
    let mut quoted = String::with_capacity(raw.len() + 2);
    quoted.push('\'');
    for ch in raw.chars() {
        if ch == '\'' {
            quoted.push_str("'\\''");
        } else {
            quoted.push(ch);
        }
    }
    quoted.push('\'');
    Ok(quoted)
}

/// Quote a command line for a Windows `REG ADD /d` value.
///
/// `reg.exe` takes `/d` as a single command-line argument; wrapping the whole
/// command in double quotes and doubling any embedded `"` keeps the path
/// intact. Newlines and NUL are rejected.
pub fn quote_windows_reg_command(exe: &Path, args: &[String]) -> AutostartResult<String> {
    let exe_raw = exe.to_str().ok_or(AutostartError::InvalidExe {
        reason: "not utf-8",
    })?;
    let mut parts = Vec::with_capacity(1 + args.len());
    parts.push(exe_raw.to_owned());
    parts.extend(args.iter().cloned());
    let joined = parts
        .into_iter()
        .map(|p| quote_windows_arg(&p))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(joined.join(" "))
}

fn quote_windows_arg(raw: &str) -> AutostartResult<String> {
    if raw.chars().any(|c| c == '\n' || c == '\r' || c == '\0') {
        return Err(AutostartError::InvalidExe {
            reason: "contains control character",
        });
    }
    if raw.is_empty() {
        return Ok("\"\"".to_owned());
    }
    let needs_quotes = raw.chars().any(|c| c == ' ' || c == '\t' || c == '"');
    if !needs_quotes {
        return Ok(raw.to_owned());
    }
    let mut out = String::with_capacity(raw.len() + 2);
    out.push('"');
    let mut backslashes = 0usize;
    for ch in raw.chars() {
        match ch {
            '\\' => backslashes += 1,
            '"' => {
                out.extend(std::iter::repeat_n('\\', backslashes * 2 + 1));
                out.push('"');
                backslashes = 0;
            }
            _ => {
                if backslashes > 0 {
                    out.extend(std::iter::repeat_n('\\', backslashes));
                    backslashes = 0;
                }
                out.push(ch);
            }
        }
    }
    if backslashes > 0 {
        out.extend(std::iter::repeat_n('\\', backslashes * 2));
    }
    out.push('"');
    Ok(out)
}

/// XML-escape a string value for a macOS LaunchAgent plist.
pub fn xml_escape(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(ch),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::*;

    #[test]
    fn systemd_exec_quotes_a_path_with_spaces() {
        let dir = std::path::PathBuf::from("/opt/My App");
        let quoted = quote_systemd_exec(&dir).expect("quote");
        assert_eq!(quoted, "\"/opt/My App\"");
        // Backslashes and quotes are escaped, not swallowed.
        let tricky = std::path::PathBuf::from(r#"/opt/My "App"\"#);
        let tricky_quoted = quote_systemd_exec(&tricky).expect("quote");
        assert_eq!(tricky_quoted, "\"/opt/My \\\"App\\\"\\\\\"");
    }

    #[test]
    fn posix_quote_escapes_a_single_quote() {
        assert_eq!(
            quote_posix_single("/etc/cron.d/db").expect("quote"),
            "'/etc/cron.d/db'"
        );
        assert_eq!(quote_posix_single("a'b").expect("q"), "'a'\\''b'");
        // A leading single quote is escaped too, not silently kept.
        assert_eq!(quote_posix_single("'leading").expect("q"), "''\\''leading'");
    }

    #[test]
    fn posix_quote_rejects_control_characters() {
        let err = quote_posix_single("a\nb").expect_err("newline");
        assert!(matches!(err, AutostartError::InvalidExe { .. }));
    }

    #[test]
    fn windows_reg_quotes_a_quoted_path_with_spaces() {
        let quoted = quote_windows_reg_command(
            std::path::Path::new("C:\\Program Files\\DBLocal\\dblocal.exe"),
            &["--autostart".to_owned()],
        )
        .expect("quote");
        assert_eq!(
            quoted, "\"C:\\Program Files\\DBLocal\\dblocal.exe\" --autostart",
            "the path with spaces stays one quoted token, flag appended"
        );
    }

    #[test]
    fn windows_reg_quotes_embedded_quotes_with_backslash_doubling() {
        // CommandLineToArgvW convention: backslashes before a quote are doubled
        // and the quote is escaped. We assert the whole shape, not just `""`.
        let quoted =
            quote_windows_reg_command(std::path::Path::new(r#"C:\My "App"\"#), &[]).expect("quote");
        assert_eq!(quoted, r#""C:\My \"App\"\\""#);
    }

    #[test]
    fn xml_escape_handles_the_five_predefined_entities() {
        let escaped = xml_escape("<A&B\"'C>");
        assert!(escaped.contains("&lt;"));
        assert!(escaped.contains("&gt;"));
        assert!(escaped.contains("&amp;"));
        assert!(escaped.contains("&quot;"));
        assert!(escaped.contains("&apos;"));
        assert!(!escaped.contains('<'));
        assert!(!escaped.contains('"'));
    }
}
