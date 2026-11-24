//! Process identity verification and SIGTERM shutdown for MariaDB.

use std::fs;
use std::path::{Path, PathBuf};
#[cfg(not(target_os = "linux"))]
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::consts;
use crate::engine::RunContext;
use crate::engine::uri::exe_name;
use crate::error::{DblError, Result};

use super::{config_path, pidfile_path};

pub(super) fn shutdown_by_sigterm(ctx: &RunContext) -> Result<()> {
    shutdown_by_sigterm_with_grace(ctx, Duration::from_secs(consts::STOP_GRACE_SECS))
}

pub(super) fn shutdown_by_sigterm_with_grace(ctx: &RunContext, grace: Duration) -> Result<()> {
    let pid_path = pidfile_path(&ctx.data_dir);
    let pid = read_pidfile(&pid_path)?;
    let identity = expected_identity(ctx);
    let verified = verify_pid_identity(pid, &identity);
    shutdown_verified_pid(pid, verified, grace)
}

pub(crate) fn shutdown_verified_pid(
    pid: u32,
    identity: PidIdentity,
    grace: Duration,
) -> Result<()> {
    match identity {
        PidIdentity::Match => {}
        PidIdentity::NoSuchProcess => {
            tracing::info!(pid, "mariadb pid already gone");
            return Ok(());
        }
        PidIdentity::NotOurs(reason) => {
            tracing::warn!(
                pid,
                reason = %reason,
                "mariadb pidfile pid is not our mariadbd; refusing to signal"
            );
            return Ok(());
        }
        PidIdentity::Unverifiable(reason) => {
            return Err(DblError::Engine {
                reason: format!(
                    "mariadb pid {pid} identity unverifiable; refusing to signal: {reason}"
                ),
            });
        }
    }

    let start = Instant::now();
    send_term(pid)?;

    while start.elapsed() < grace {
        if !pid_alive(pid) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(200));
    }

    if !pid_alive(pid) {
        return Ok(());
    }
    Err(DblError::Engine {
        reason: format!(
            "mariadbd pid {pid} did not exit within {}s after SIGTERM",
            grace.as_secs()
        ),
    })
}

pub(crate) fn read_pidfile(path: &Path) -> Result<u32> {
    let contents = fs::read_to_string(path).map_err(|e| DblError::Engine {
        reason: format!("cannot read pidfile {}: {e}", path.display()),
    })?;
    let pid = contents
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .ok_or_else(|| DblError::Engine {
            reason: format!("pidfile {} is empty", path.display()),
        })?
        .parse::<u32>()
        .map_err(|_| DblError::Engine {
            reason: format!("pidfile {} does not contain a pid", path.display()),
        })?;
    Ok(pid)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExpectedIdentity {
    pub(crate) exe: PathBuf,
    pub(crate) data_dir: PathBuf,
    pub(crate) config_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PidIdentity {
    Match,
    NoSuchProcess,
    NotOurs(String),
    Unverifiable(String),
}

pub(crate) fn expected_identity(ctx: &RunContext) -> ExpectedIdentity {
    ExpectedIdentity {
        exe: ctx.runtime_dir.join("bin").join(exe_name("mariadbd")),
        data_dir: ctx.data_dir.clone(),
        config_path: config_path(&ctx.data_dir),
    }
}

pub(crate) fn verify_pid_identity(pid: u32, expected: &ExpectedIdentity) -> PidIdentity {
    #[cfg(target_os = "linux")]
    {
        verify_pid_identity_linux(pid, expected)
    }
    #[cfg(not(target_os = "linux"))]
    {
        verify_pid_identity_fallback(pid, expected)
    }
}

#[cfg(target_os = "linux")]
fn verify_pid_identity_linux(pid: u32, expected: &ExpectedIdentity) -> PidIdentity {
    let proc_dir = PathBuf::from(format!("/proc/{pid}"));
    if !proc_dir.exists() {
        return PidIdentity::NoSuchProcess;
    }
    if is_zombie(pid) {
        return PidIdentity::NoSuchProcess;
    }
    let exe_link = proc_dir.join("exe");
    let observed_exe = match fs::read_link(&exe_link) {
        Ok(p) => p,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return PidIdentity::NoSuchProcess,
        Err(e) => return PidIdentity::Unverifiable(format!("cannot read exe: {e}")),
    };
    if !same_executable(&observed_exe, &expected.exe) {
        return PidIdentity::NotOurs(format!(
            "exe {} != {}",
            observed_exe.display(),
            expected.exe.display()
        ));
    }
    let cmdline_path = proc_dir.join("cmdline");
    let cmdline_bytes = match fs::read(&cmdline_path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return PidIdentity::NoSuchProcess,
        Err(e) => return PidIdentity::Unverifiable(format!("cannot read cmdline: {e}")),
    };
    let args = parse_cmdline(&cmdline_bytes);
    if !cmdline_targets_instance(&args, expected) {
        return PidIdentity::NotOurs("cmdline does not target this instance".to_owned());
    }
    PidIdentity::Match
}

#[cfg(not(target_os = "linux"))]
fn verify_pid_identity_fallback(pid: u32, expected: &ExpectedIdentity) -> PidIdentity {
    if !pid_alive(pid) {
        return PidIdentity::NoSuchProcess;
    }
    let out = Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "command="])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output();
    let Ok(out) = out else {
        return PidIdentity::Unverifiable("ps failed".to_owned());
    };
    if !out.status.success() {
        return PidIdentity::NoSuchProcess;
    }
    let cmd = String::from_utf8_lossy(&out.stdout);
    verify_ps_command_output(cmd.trim(), expected)
}

#[cfg(any(test, not(target_os = "linux")))]
pub(crate) fn verify_ps_command_output(cmd: &str, expected: &ExpectedIdentity) -> PidIdentity {
    if cmd.is_empty() {
        return PidIdentity::NoSuchProcess;
    }
    let expected_exe = expected.exe.to_string_lossy();
    let Some(rest) = command_starts_with_executable(cmd, &expected_exe) else {
        let observed = cmd.split_whitespace().next().unwrap_or(cmd);
        return PidIdentity::NotOurs(format!("exe {} != {}", observed, expected.exe.display()));
    };
    let args_tail = rest.trim_start();
    if !ps_command_targets_instance(args_tail, expected) {
        return PidIdentity::NotOurs("cmdline does not target this instance".to_owned());
    }
    PidIdentity::Match
}

#[cfg(any(test, not(target_os = "linux")))]
fn command_starts_with_executable<'a>(cmd: &'a str, expected_exe: &str) -> Option<&'a str> {
    let rest = cmd.strip_prefix(expected_exe)?;
    if rest.is_empty() || rest.starts_with(char::is_whitespace) {
        return Some(rest);
    }
    None
}

#[cfg(any(test, not(target_os = "linux")))]
fn ps_command_targets_instance(args_tail: &str, expected: &ExpectedIdentity) -> bool {
    let datadir = expected.data_dir.to_string_lossy();
    let conf = expected.config_path.to_string_lossy();
    let datadir_arg = format!("--datadir={datadir}");
    let conf_arg = format!("--defaults-file={conf}");
    contains_arg_token(args_tail, &datadir_arg) || contains_arg_token(args_tail, &conf_arg)
}

#[cfg(any(test, not(target_os = "linux")))]
pub(crate) fn contains_arg_token(haystack: &str, token: &str) -> bool {
    let mut offset = 0;
    while let Some(pos) = haystack[offset..].find(token) {
        let start = offset + pos;
        let end = start + token.len();
        let before_ok = start == 0 || haystack.as_bytes()[start - 1].is_ascii_whitespace();
        let after_ok = end == haystack.len() || haystack.as_bytes()[end].is_ascii_whitespace();
        if before_ok && after_ok {
            return true;
        }
        offset = start + 1;
        if offset >= haystack.len() {
            break;
        }
    }
    false
}

/// Linux `/proc` identity helpers. Only `verify_pid_identity_linux` calls them in
/// a non-test build, so they must not exist on other targets (clippy runs with
/// `-D warnings`, and a dead `pub(crate)` item is an error there). Tests use them
/// on every host to pin the parsers without `/proc`.
#[cfg(any(target_os = "linux", test))]
pub(crate) fn same_executable(observed: &Path, expected: &Path) -> bool {
    let observed = strip_deleted(observed);
    let expected = strip_deleted(expected);
    if observed == expected {
        return true;
    }
    let obs_name = observed.file_name().and_then(|s| s.to_str()).unwrap_or("");
    let exp_name = expected.file_name().and_then(|s| s.to_str()).unwrap_or("");
    if obs_name != exp_name || obs_name.is_empty() {
        return false;
    }
    // Canonicalize when possible; if either fails, fall back to name match only
    // when the observed path ends with the expected suffix.
    match (observed.canonicalize(), expected.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => observed.ends_with(expected.as_ref()) || expected.ends_with(observed.as_ref()),
    }
}

#[cfg(any(target_os = "linux", test))]
pub(crate) fn strip_deleted(path: &Path) -> std::borrow::Cow<'_, Path> {
    #[cfg(unix)]
    {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;
        const SUFFIX: &[u8] = b" (deleted)";
        let bytes = path.as_os_str().as_bytes();
        if let Some(stripped) = bytes.strip_suffix(SUFFIX) {
            return std::borrow::Cow::Owned(PathBuf::from(OsStr::from_bytes(stripped)));
        }
    }
    std::borrow::Cow::Borrowed(path)
}

#[cfg(any(target_os = "linux", test))]
pub(crate) fn parse_cmdline(bytes: &[u8]) -> Vec<String> {
    bytes
        .split(|b| *b == 0)
        .filter(|s| !s.is_empty())
        .map(|s| String::from_utf8_lossy(s).into_owned())
        .collect()
}

#[cfg(any(target_os = "linux", test))]
pub(crate) fn cmdline_targets_instance(args: &[String], expected: &ExpectedIdentity) -> bool {
    let datadir = expected.data_dir.to_string_lossy();
    let conf = expected.config_path.to_string_lossy();
    let mut saw_datadir = false;
    let mut saw_conf = false;
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if let Some(v) = a.strip_prefix("--datadir=") {
            saw_datadir = path_arg_matches(v, &datadir);
        } else if a == "--datadir"
            && let Some(v) = args.get(i + 1)
        {
            saw_datadir = path_arg_matches(v, &datadir);
        } else if let Some(v) = a.strip_prefix("--defaults-file=") {
            saw_conf = path_arg_matches(v, &conf);
        } else if a == "--defaults-file"
            && let Some(v) = args.get(i + 1)
        {
            saw_conf = path_arg_matches(v, &conf);
        }
        i += 1;
    }
    saw_datadir || saw_conf
}

#[cfg(any(target_os = "linux", test))]
pub(crate) fn path_arg_matches(arg: &str, expected: &str) -> bool {
    if arg == expected {
        return true;
    }
    let a = Path::new(arg);
    let b = Path::new(expected);
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

#[cfg(any(target_os = "linux", test))]
pub(crate) fn is_zombie_stat(stat: &str) -> bool {
    let Some(close) = stat.rfind(')') else {
        return false;
    };
    let rest = &stat[close + 1..];
    let mut parts = rest.split_whitespace();
    matches!(parts.next(), Some("Z"))
}

#[cfg(target_os = "linux")]
pub(super) fn is_zombie(pid: u32) -> bool {
    let stat_path = format!("/proc/{pid}/stat");
    let Ok(stat) = fs::read_to_string(stat_path) else {
        return false;
    };
    is_zombie_stat(&stat)
}

#[cfg(unix)]
fn send_term(pid: u32) -> Result<()> {
    use nix::sys::signal::{Signal, kill};
    use nix::unistd::Pid;
    match kill(Pid::from_raw(pid as i32), Signal::SIGTERM) {
        Ok(()) => Ok(()),
        Err(nix::errno::Errno::ESRCH) => Ok(()),
        Err(e) => Err(DblError::Engine {
            reason: format!("failed to send SIGTERM to mariadbd pid {pid}: {e}"),
        }),
    }
}

#[cfg(not(unix))]
fn send_term(pid: u32) -> Result<()> {
    let _ = pid;
    Err(DblError::Unsupported {
        reason: "mariadb SIGTERM shutdown is not implemented on this platform".to_owned(),
    })
}

#[cfg(unix)]
pub(crate) fn pid_alive(pid: u32) -> bool {
    use nix::sys::signal::kill;
    use nix::unistd::Pid;
    #[cfg(target_os = "linux")]
    if is_zombie(pid) {
        return false;
    }
    match kill(Pid::from_raw(pid as i32), None) {
        Ok(()) => true,
        Err(nix::errno::Errno::EPERM) => true, // exists but no permission; treat as alive
        Err(_) => false,
    }
}

/// Windows has no signal 0, and the workspace denies `unsafe_code` outside the
/// process module, so liveness comes from the process table via `sysinfo` — the
/// same path `process::probe` and the Windows terminate path already use. A PID
/// that is absent, or that the table marks dead/zombie, is not alive.
///
/// This used to return `false` unconditionally, which made the MariaDB stop path
/// believe every Windows server was already gone: `verify_pid_identity` reports
/// `NoSuchProcess` for a live server and the grace loop never waits.
#[cfg(windows)]
pub(crate) fn pid_alive(pid: u32) -> bool {
    let Ok(mut system) = sysinfo::System::new() else {
        // Without a process table nothing can be proven; "not alive" makes the
        // caller treat the PID as gone instead of waiting out the whole grace.
        return false;
    };
    let target = sysinfo::Pid::from_u32(pid);
    system.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[target]), true);
    match system.process(target) {
        Some(process) => !matches!(
            process.status(),
            sysinfo::ProcessStatus::Zombie | sysinfo::ProcessStatus::Dead
        ),
        None => false,
    }
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn pid_alive(pid: u32) -> bool {
    let _ = pid;
    false
}
