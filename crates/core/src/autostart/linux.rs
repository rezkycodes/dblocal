//! Linux login items: a `systemd --user` unit with an XDG autostart fallback.
//!
//! PRD `F-AUT-01` AC1 asks for a user-level mechanism with no sudo. The primary
//! path is `$XDG_CONFIG_HOME/systemd/user/dblocal.service` enabled through
//! `systemctl --user`; when the machine has no user systemd (containers, some
//! WMs, WSL without systemd) the same login item is written as
//! `$XDG_CONFIG_HOME/autostart/dblocal.desktop`, which every XDG autostart
//! implementation understands.
//!
//! Nothing here touches a real file outside the injected `config_home`, and
//! every helper command goes through the injected [`CommandRunner`].

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::backend::{
    AutostartBackend, CommandRunner, LoginItem, LoginItemStatus, quote_systemd_exec,
};
use super::consts;
use super::error::{AutostartError, AutostartResult};

/// `systemd --user` unit under `$XDG_CONFIG_HOME/systemd/user/`.
pub struct LinuxBackend {
    unit_path: PathBuf,
    /// `.wants` directories `enable` may have created a symlink in. `enable`
    /// uses `graphical-session.target` (D3), but a unit enabled by an older
    /// build lived under `default.target.wants`, so both are recognised.
    wants_dirs: Vec<PathBuf>,
    desktop_path: PathBuf,
    runner: Arc<dyn CommandRunner>,
}

impl LinuxBackend {
    /// Builds the backend for `config_home` (the XDG config root).
    pub fn new(_home: &Path, config_home: &Path, runner: Arc<dyn CommandRunner>) -> Self {
        let user_dir = config_home.join("systemd").join("user");
        Self {
            unit_path: user_dir.join(consts::SYSTEMD_UNIT_FILE),
            wants_dirs: vec![
                user_dir.join("graphical-session.target.wants"),
                user_dir.join("default.target.wants"),
            ],
            desktop_path: config_home.join("autostart").join(consts::XDG_DESKTOP_FILE),
            runner,
        }
    }

    /// Absolute path of the systemd unit file.
    pub fn unit_path(&self) -> &Path {
        &self.unit_path
    }

    /// The primary `.wants` directory `enable` targets (graphical session, D3).
    pub fn wants_dir(&self) -> &Path {
        &self.wants_dirs[0]
    }

    /// Absolute path of the XDG desktop file.
    pub fn desktop_path(&self) -> &Path {
        &self.desktop_path
    }

    /// Whether a user systemd manager answers on this machine.
    ///
    /// Asked of `systemctl --user` itself rather than guessed from the
    /// environment: a login item that `enable`s a unit on a machine whose user
    /// manager never starts would be silently ignored.
    pub fn has_user_systemd(&self) -> bool {
        match self.runner.run(
            consts::SYSTEMCTL,
            &["--user".to_owned(), "show-environment".to_owned()],
        ) {
            Ok(output) => output.success(),
            Err(_) => false,
        }
    }

    fn systemctl(&self, args: &[&str]) -> AutostartResult<()> {
        let mut owned: Vec<String> = vec!["--user".to_owned()];
        owned.extend(args.iter().map(|s| (*s).to_owned()));
        let output = self.runner.run(consts::SYSTEMCTL, &owned)?;
        if output.success() {
            Ok(())
        } else {
            Err(AutostartError::CommandFailed {
                program: consts::SYSTEMCTL,
                detail: if output.stderr.is_empty() {
                    format!("exit {}", output.code)
                } else {
                    output.stderr
                },
            })
        }
    }
}

impl AutostartBackend for LinuxBackend {
    fn enable(&self, item: &LoginItem) -> AutostartResult<LoginItemStatus> {
        if self.has_user_systemd() {
            // D2: remember what was there before this call so a failed enable can
            // restore the machine exactly as it was (no half-written unit that
            // `status` would then report as enabled).
            let previous_unit = std::fs::read(&self.unit_path).ok();
            self.write_unit(item)?;
            // `daemon-reload` is required before a fresh unit can be enabled;
            // a failure means the unit is not actually registered, so it is
            // reported and rolled back rather than swallowed.
            let reload = self.systemctl(&["daemon-reload"]);
            let enabled =
                reload.and_then(|()| self.systemctl(&["enable", consts::SYSTEMD_UNIT_ARG]));
            if let Err(err) = enabled {
                self.restore_unit(previous_unit.as_deref());
                let _ = self.systemctl(&["daemon-reload"]);
                return Err(err);
            }
            // A stale desktop entry from an earlier fallback would double-start
            // the app, so it is removed once the unit is in place.
            remove_if_present(&self.desktop_path)?;
            return Ok(LoginItemStatus {
                enabled: true,
                location: self.unit_path.clone(),
            });
        }
        self.write_desktop(item)?;
        Ok(LoginItemStatus {
            enabled: true,
            location: self.desktop_path.clone(),
        })
    }

    fn disable(&self) -> AutostartResult<LoginItemStatus> {
        let had_unit = self.unit_path.exists();
        if had_unit {
            // Best effort: `systemctl --user disable` removes the wants symlink
            // through systemd. When no manager is running it fails, so the
            // symlink is removed by hand below; a refusal here must not leave a
            // half-removed login item behind.
            let _ = self.systemctl(&["disable", consts::SYSTEMD_UNIT_ARG]);
            for wants_dir in &self.wants_dirs {
                remove_wants_symlink(wants_dir, &self.unit_path);
            }
            remove_if_present(&self.unit_path)?;
            let _ = self.systemctl(&["daemon-reload"]);
        }
        remove_if_present(&self.desktop_path)?;
        Ok(LoginItemStatus {
            enabled: false,
            location: if had_unit {
                self.unit_path.clone()
            } else {
                self.desktop_path.clone()
            },
        })
    }

    fn status(&self) -> AutostartResult<LoginItemStatus> {
        // D2: a unit file alone is not "enabled" — it is a dead file when the
        // `enable` (or its rollback) never registered it. Count the unit as the
        // mechanism only when its `default.target.wants`-style symlink exists;
        // otherwise fall back to the presence of the unit file only to name the
        // location, but report `enabled: false`.
        let unit_present = self.unit_path.exists();
        let unit_enabled = unit_present
            && self
                .wants_dirs
                .iter()
                .any(|dir| wants_link_exists(dir, &self.unit_path));
        let (enabled, location) = if unit_enabled {
            (true, self.unit_path.clone())
        } else if self.desktop_path.exists() {
            (true, self.desktop_path.clone())
        } else if unit_present {
            // A unit file with no wants symlink: not enabled. Report the path so
            // the user can see what `disable` will clean up.
            (false, self.unit_path.clone())
        } else {
            (false, self.unit_path.clone())
        };
        Ok(LoginItemStatus { enabled, location })
    }
}

impl LinuxBackend {
    /// Restores the unit file to what it was before a failed `enable`.
    fn restore_unit(&self, previous: Option<&[u8]>) {
        match previous {
            Some(bytes) => {
                let _ = write_file(&self.unit_path, &String::from_utf8_lossy(bytes));
            }
            None => {
                let _ = remove_if_present(&self.unit_path);
            }
        }
    }
}

impl LinuxBackend {
    fn write_unit(&self, item: &LoginItem) -> AutostartResult<()> {
        let exec = systemd_exec_line(item)?;
        // D3: bind to the graphical session, not `default.target`. A unit on
        // `default.target` starts at the first login session, before the
        // compositor imports `DISPLAY`/`WAYLAND_DISPLAY` into the user manager,
        // so the app would take the headless path instead of the tray path
        // (F-AUT-01 AC3). `After=`/`PartOf=` order it with the session, and
        // `WantedBy=graphical-session.target` pulls it in only once the session
        // exists. Window managers without systemd session integration (plain
        // i3/sway) never activate that target, so on those the XDG `.desktop`
        // fallback is the portable mechanism; see docs/architecture/autostart.md.
        let body = format!(
            "[Unit]\n\
             Description=DBLocal local database manager\n\
             After=graphical-session.target\n\
             PartOf=graphical-session.target\n\
             \n\
             [Service]\n\
             Type=simple\n\
             ExecStart={exec}\n\
             Restart=no\n\
             \n\
             [Install]\n\
             WantedBy=graphical-session.target\n"
        );
        write_file(&self.unit_path, &body)
    }

    fn write_desktop(&self, item: &LoginItem) -> AutostartResult<()> {
        let exec = desktop_exec_line(item)?;
        let body = format!(
            "[Desktop Entry]\n\
             Type=Application\n\
             Name={}\n\
             Comment=DBLocal local database manager\n\
             Exec={exec}\n\
             Terminal=false\n\
             X-GNOME-Autostart-enabled=true\n",
            consts::DESKTOP_NAME
        );
        write_file(&self.desktop_path, &body)
    }
}

/// Builds the `ExecStart=` value for the systemd unit.
fn systemd_exec_line(item: &LoginItem) -> AutostartResult<String> {
    let mut parts = Vec::with_capacity(1 + item.args.len());
    parts.push(quote_systemd_exec(&item.exe)?);
    for arg in &item.args {
        // Flags such as `--autostart` contain no whitespace, but quoting is
        // uniform across the line so a future argument with a space is safe.
        parts.push(quote_systemd_exec(Path::new(arg))?);
    }
    Ok(parts.join(" "))
}

/// Builds the `Exec=` value for the XDG desktop entry.
///
/// The desktop-entry spec reserves `%` for field codes and requires it to be
/// doubled, and quoting is by double quotes with backslash escapes. Single
/// quotes are accepted by the common implementations but not by the spec, so
/// this path stays spec-shaped.
fn desktop_exec_line(item: &LoginItem) -> AutostartResult<String> {
    let mut parts = Vec::with_capacity(1 + item.args.len());
    parts.push(quote_desktop_exec(&item.exe.display().to_string())?);
    for arg in &item.args {
        parts.push(quote_desktop_exec(arg)?);
    }
    Ok(parts.join(" "))
}

fn quote_desktop_exec(raw: &str) -> AutostartResult<String> {
    if raw.chars().any(|c| c == '\n' || c == '\r' || c == '\0') {
        return Err(AutostartError::InvalidExe {
            reason: "contains control character",
        });
    }
    let mut quoted = String::with_capacity(raw.len() + 2);
    quoted.push('"');
    for ch in raw.chars() {
        match ch {
            '%' => quoted.push_str("%%"),
            '"' | '\\' | '`' | '$' => {
                quoted.push('\\');
                quoted.push(ch);
            }
            _ => quoted.push(ch),
        }
    }
    quoted.push('"');
    Ok(quoted)
}

fn write_file(path: &Path, body: &str) -> AutostartResult<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|_| AutostartError::WriteFailed {
            path: display(path),
        })?;
    }
    fs::write(path, body).map_err(|_| AutostartError::WriteFailed {
        path: display(path),
    })
}

fn remove_if_present(path: &Path) -> AutostartResult<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(AutostartError::WriteFailed {
            path: display(path),
        }),
    }
}

/// Whether any symlink in `wants_dir` points at `unit_path` (D2).
///
/// This is what makes a unit file actually *enabled*: `systemctl --user enable`
/// creates the `.wants` symlink; a bare unit file left by a failed enable is not
/// a live login item.
fn wants_link_exists(wants_dir: &Path, unit_path: &Path) -> bool {
    let dir = match fs::read_dir(wants_dir) {
        Ok(dir) => dir,
        Err(_) => return false,
    };
    for entry in dir.flatten() {
        let path = entry.path();
        let Ok(meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        if !meta.file_type().is_symlink() {
            continue;
        }
        let Ok(target) = fs::read_link(&path) else {
            continue;
        };
        let points_at_unit = target == unit_path
            || target.file_name() == unit_path.file_name()
            || target.ends_with(unit_path.file_name().unwrap_or_default());
        if points_at_unit {
            return true;
        }
    }
    false
}

/// Removes a `.wants` symlink pointing at our unit.
///
/// Only a symlink whose target is our unit file is removed, so no other entry
/// in that directory can be lost.
fn remove_wants_symlink(wants_dir: &Path, unit_path: &Path) {
    let dir = match fs::read_dir(wants_dir) {
        Ok(dir) => dir,
        Err(_) => return,
    };
    for entry in dir.flatten() {
        let path = entry.path();
        let meta = match fs::symlink_metadata(&path) {
            Ok(meta) => meta,
            Err(_) => continue,
        };
        if !meta.file_type().is_symlink() {
            continue;
        }
        let target = match fs::read_link(&path) {
            Ok(target) => target,
            Err(_) => continue,
        };
        let points_at_unit = target == unit_path
            || target.file_name() == unit_path.file_name()
            || target.ends_with(unit_path.file_name().unwrap_or_default());
        if points_at_unit {
            let _ = fs::remove_file(&path);
        }
    }
}

fn display(path: &Path) -> String {
    path.display().to_string()
}

// The backend type is compiled on every host (so `backend_for(Os::Linux, ...)`
// keeps type-checking), but its behaviour is a POSIX one: systemd `.wants`
// symlinks and `/`-rooted XDG paths. Windows has neither, so its filesystem
// tests cannot assert anything true there and are only built on unix.
#[cfg(all(test, unix))]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use std::collections::HashMap;
    use std::sync::Mutex;

    use super::*;
    use crate::autostart::backend::CommandOutput;
    use tempfile::tempdir;

    /// Records every helper call and answers from a scripted table.
    struct FakeRunner {
        answers: Mutex<HashMap<String, CommandOutput>>,
        calls: Mutex<Vec<(String, Vec<String>)>>,
    }

    impl FakeRunner {
        fn new() -> Self {
            Self {
                answers: Mutex::new(HashMap::new()),
                calls: Mutex::new(Vec::new()),
            }
        }

        fn with(self, key: &str, output: CommandOutput) -> Self {
            self.answers
                .lock()
                .expect("lock")
                .insert(key.to_owned(), output);
            self
        }

        fn calls(&self) -> Vec<(String, Vec<String>)> {
            self.calls.lock().expect("lock").clone()
        }
    }

    impl CommandRunner for FakeRunner {
        fn run(&self, program: &str, args: &[String]) -> AutostartResult<CommandOutput> {
            self.calls
                .lock()
                .expect("lock")
                .push((program.to_owned(), args.to_vec()));
            let key = format!("{program} {}", args.join(" "));
            let answer = self.answers.lock().expect("lock").get(&key).cloned();
            Ok(answer.unwrap_or_else(|| CommandOutput {
                code: 1,
                stdout: String::new(),
                stderr: "not scripted".to_owned(),
            }))
        }
    }

    fn item(dir: &Path) -> LoginItem {
        LoginItem::for_exe(dir.join("Application Support").join("dblocal")).expect("item")
    }

    #[test]
    fn systemd_present_writes_a_unit_and_enables_it() {
        let dir = tempdir().expect("tempdir");
        let config = dir.path().join("config");
        let runner = Arc::new(
            FakeRunner::new()
                .with("systemctl --user show-environment", CommandOutput::ok())
                .with("systemctl --user daemon-reload", CommandOutput::ok())
                .with(
                    "systemctl --user enable dblocal.service",
                    CommandOutput::ok(),
                ),
        );
        let backend = LinuxBackend::new(dir.path(), &config, runner.clone());
        let status = backend.enable(&item(dir.path())).expect("enable");
        assert!(status.enabled);
        assert_eq!(status.location, backend.unit_path());

        let body = fs::read_to_string(backend.unit_path()).expect("unit written");
        assert!(body.contains("[Service]"));
        // D3: the unit binds to the graphical session, not default.target.
        assert!(body.contains("WantedBy=graphical-session.target"));
        assert!(body.contains("After=graphical-session.target"));
        assert!(body.contains("PartOf=graphical-session.target"));
        assert!(!body.contains("default.target"));
        assert!(
            body.contains("ExecStart=\""),
            "the exec line must be quoted: {body}"
        );
        assert!(body.contains("Application Support"), "path kept: {body}");
        assert!(body.contains("--autostart"), "flag kept: {body}");
        let calls = runner.calls();
        assert!(
            calls
                .iter()
                .any(|(p, a)| p == "systemctl" && a.join(" ") == "--user enable dblocal.service"),
            "enable was never called: {calls:?}"
        );
    }

    #[test]
    fn a_path_with_spaces_is_quoted_inside_the_unit() {
        let dir = tempdir().expect("tempdir");
        let config = dir.path().join("config");
        let runner = Arc::new(
            FakeRunner::new()
                .with("systemctl --user show-environment", CommandOutput::ok())
                .with("systemctl --user daemon-reload", CommandOutput::ok())
                .with(
                    "systemctl --user enable dblocal.service",
                    CommandOutput::ok(),
                ),
        );
        let backend = LinuxBackend::new(dir.path(), &config, runner);
        backend.enable(&item(dir.path())).expect("enable");
        let body = fs::read_to_string(backend.unit_path()).expect("unit");
        let exec = body
            .lines()
            .find(|line| line.starts_with("ExecStart="))
            .expect("exec line");
        assert_eq!(
            exec,
            format!(
                "ExecStart=\"{}/Application Support/dblocal\" \"--autostart\"",
                dir.path().display()
            )
        );
    }

    #[test]
    fn no_user_systemd_falls_back_to_a_desktop_entry() {
        let dir = tempdir().expect("tempdir");
        let config = dir.path().join("config");
        let runner = Arc::new(FakeRunner::new());
        let backend = LinuxBackend::new(dir.path(), &config, runner.clone());
        let status = backend.enable(&item(dir.path())).expect("enable");
        assert!(status.enabled);
        assert_eq!(status.location, backend.desktop_path());
        assert!(!backend.unit_path().exists(), "no unit without systemd");
        let body = fs::read_to_string(backend.desktop_path()).expect("desktop written");
        assert!(body.contains("[Desktop Entry]"));
        assert!(body.contains("X-GNOME-Autostart-enabled=true"));
        assert!(body.contains("--autostart"));
        let systemctl_calls: Vec<_> = runner
            .calls()
            .into_iter()
            .filter(|(program, _)| program == "systemctl")
            .collect();
        assert!(
            systemctl_calls
                .iter()
                .all(|(_, args)| args.first().map(String::as_str) == Some("--user")),
            "without a manager only the --user probe may run: {systemctl_calls:?}"
        );
    }

    #[test]
    fn a_percent_in_the_exec_is_escaped_for_the_desktop_entry() {
        let dir = tempdir().expect("tempdir");
        let config = dir.path().join("config");
        let backend = LinuxBackend::new(dir.path(), &config, Arc::new(FakeRunner::new()));
        let exe = dir.path().join("100% local").join("dblocal");
        let item = LoginItem::for_exe(exe).expect("item");
        backend.enable(&item).expect("enable");
        let body = fs::read_to_string(backend.desktop_path()).expect("desktop");
        assert!(
            body.contains("100%% local"),
            "a literal % must be escaped: {body}"
        );
    }

    #[test]
    fn disable_removes_the_unit_the_wants_link_and_the_desktop_file() {
        let dir = tempdir().expect("tempdir");
        let config = dir.path().join("config");
        let runner = Arc::new(
            FakeRunner::new()
                .with("systemctl --user show-environment", CommandOutput::ok())
                .with("systemctl --user daemon-reload", CommandOutput::ok())
                .with(
                    "systemctl --user enable dblocal.service",
                    CommandOutput::ok(),
                ),
        );
        let backend = LinuxBackend::new(dir.path(), &config, runner);
        backend.enable(&item(dir.path())).expect("enable");
        // Simulate the symlink systemd would create under the graphical session.
        let wants_dir = backend.wants_dir().to_path_buf();
        fs::create_dir_all(&wants_dir).expect("wants dir");
        let link = wants_dir.join(consts::SYSTEMD_UNIT_FILE);
        #[cfg(unix)]
        std::os::unix::fs::symlink(backend.unit_path(), &link).expect("symlink");
        let stray = wants_dir.join("unrelated.service");
        #[cfg(unix)]
        std::os::unix::fs::symlink("/tmp/other.service", &stray).expect("symlink");

        let status = backend.disable().expect("disable");
        assert!(!status.enabled);
        assert!(!backend.unit_path().exists(), "unit left behind");
        assert!(!link.exists(), "wants symlink left behind");
        assert!(
            fs::symlink_metadata(&stray).is_ok(),
            "an unrelated wants entry must survive"
        );
    }

    #[test]
    fn disable_is_idempotent() {
        let dir = tempdir().expect("tempdir");
        let config = dir.path().join("config");
        let backend = LinuxBackend::new(dir.path(), &config, Arc::new(FakeRunner::new()));
        let first = backend.disable().expect("first disable");
        let second = backend.disable().expect("second disable");
        assert!(!first.enabled);
        assert!(!second.enabled);
        assert!(!backend.unit_path().exists());
        assert!(!backend.desktop_path().exists());
    }

    #[test]
    fn status_reports_the_mechanism_in_use() {
        let dir = tempdir().expect("tempdir");
        let config = dir.path().join("config");
        let backend = LinuxBackend::new(dir.path(), &config, Arc::new(FakeRunner::new()));
        assert!(!backend.status().expect("status").enabled);
        backend.enable(&item(dir.path())).expect("enable");
        let status = backend.status().expect("status");
        assert!(status.enabled);
        assert_eq!(status.location, backend.desktop_path());
    }

    #[test]
    fn enable_removes_a_stale_desktop_entry_once_systemd_is_used() {
        let dir = tempdir().expect("tempdir");
        let config = dir.path().join("config");
        // First write the fallback, then enable again with systemd available.
        let fallback = LinuxBackend::new(dir.path(), &config, Arc::new(FakeRunner::new()));
        fallback.enable(&item(dir.path())).expect("fallback enable");
        assert!(fallback.desktop_path().exists());

        let runner = Arc::new(
            FakeRunner::new()
                .with("systemctl --user show-environment", CommandOutput::ok())
                .with("systemctl --user daemon-reload", CommandOutput::ok())
                .with(
                    "systemctl --user enable dblocal.service",
                    CommandOutput::ok(),
                ),
        );
        let systemd = LinuxBackend::new(dir.path(), &config, runner);
        systemd.enable(&item(dir.path())).expect("systemd enable");
        assert!(systemd.unit_path().exists());
        assert!(
            !systemd.desktop_path().exists(),
            "a stale fallback would start the app twice"
        );
    }

    #[test]
    fn a_failing_enable_command_is_reported() {
        let dir = tempdir().expect("tempdir");
        let config = dir.path().join("config");
        let runner = Arc::new(
            FakeRunner::new()
                .with("systemctl --user show-environment", CommandOutput::ok())
                .with("systemctl --user daemon-reload", CommandOutput::ok())
                .with(
                    "systemctl --user enable dblocal.service",
                    CommandOutput {
                        code: 1,
                        stdout: String::new(),
                        stderr: "Failed to connect to bus".to_owned(),
                    },
                ),
        );
        let backend = LinuxBackend::new(dir.path(), &config, runner);
        let err = backend.enable(&item(dir.path())).expect_err("must fail");
        assert_eq!(err.code(), super::super::error::CODE_COMMAND_FAILED);
    }

    #[test]
    fn a_failed_enable_rolls_back_a_newly_written_unit() {
        // D2: the failing `enable` must not leave a unit file behind, or
        // `status` would claim the login item exists.
        let dir = tempdir().expect("tempdir");
        let config = dir.path().join("config");
        let runner = Arc::new(
            FakeRunner::new()
                .with("systemctl --user show-environment", CommandOutput::ok())
                .with("systemctl --user daemon-reload", CommandOutput::ok())
                .with(
                    "systemctl --user enable dblocal.service",
                    CommandOutput {
                        code: 1,
                        stdout: String::new(),
                        stderr: "Failed to connect to bus".to_owned(),
                    },
                ),
        );
        let backend = LinuxBackend::new(dir.path(), &config, runner);
        let _ = backend.enable(&item(dir.path())).expect_err("must fail");
        assert!(
            !backend.unit_path().exists(),
            "a failed enable left the unit behind"
        );
        assert!(
            !backend.status().expect("status").enabled,
            "status must not report enabled after a failed enable"
        );
    }

    #[test]
    fn a_failed_enable_restores_a_previously_present_unit() {
        // D2: if a unit already existed (an earlier good enable), a later failed
        // enable restores its content instead of deleting it.
        let dir = tempdir().expect("tempdir");
        let config = dir.path().join("config");
        let backend = LinuxBackend::new(dir.path(), &config, Arc::new(FakeRunner::new()));
        fs::create_dir_all(backend.unit_path().parent().expect("parent")).expect("dir");
        let previous = "[Unit]\nDescription=previous\n";
        fs::write(backend.unit_path(), previous).expect("seed unit");

        let runner = Arc::new(
            FakeRunner::new()
                .with("systemctl --user show-environment", CommandOutput::ok())
                .with("systemctl --user daemon-reload", CommandOutput::ok())
                .with(
                    "systemctl --user enable dblocal.service",
                    CommandOutput {
                        code: 1,
                        stdout: String::new(),
                        stderr: "Failed to connect to bus".to_owned(),
                    },
                ),
        );
        let failing = LinuxBackend::new(dir.path(), &config, runner);
        let _ = failing.enable(&item(dir.path())).expect_err("must fail");
        let restored = fs::read_to_string(failing.unit_path()).expect("unit restored");
        assert_eq!(restored, previous, "the previous unit content must return");
    }

    #[test]
    fn status_is_not_enabled_for_a_unit_without_a_wants_symlink() {
        // D2: a bare unit file (left by a crash or a hand copy) is not enabled.
        let dir = tempdir().expect("tempdir");
        let config = dir.path().join("config");
        let backend = LinuxBackend::new(dir.path(), &config, Arc::new(FakeRunner::new()));
        fs::create_dir_all(backend.unit_path().parent().expect("parent")).expect("dir");
        fs::write(backend.unit_path(), "[Unit]\n").expect("unit file");
        let status = backend.status().expect("status");
        assert!(
            !status.enabled,
            "a unit without a wants symlink is not enabled"
        );
        assert_eq!(status.location, backend.unit_path());
    }

    #[test]
    fn status_is_enabled_when_the_wants_symlink_exists() {
        let dir = tempdir().expect("tempdir");
        let config = dir.path().join("config");
        let backend = LinuxBackend::new(dir.path(), &config, Arc::new(FakeRunner::new()));
        fs::create_dir_all(backend.unit_path().parent().expect("parent")).expect("dir");
        fs::write(backend.unit_path(), "[Unit]\n").expect("unit file");
        let wants = backend.wants_dir().to_path_buf();
        fs::create_dir_all(&wants).expect("wants");
        #[cfg(unix)]
        std::os::unix::fs::symlink(backend.unit_path(), wants.join(consts::SYSTEMD_UNIT_FILE))
            .expect("symlink");
        let status = backend.status().expect("status");
        assert!(status.enabled);
        assert_eq!(status.location, backend.unit_path());
    }

    #[test]
    fn relative_executables_are_refused() {
        let err = LoginItem::for_exe("dblocal").expect_err("relative path");
        assert_eq!(err.code(), super::super::error::CODE_INVALID_EXE);
    }
}
