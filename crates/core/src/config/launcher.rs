//! Opening the raw config file in the OS default handler (`F-CFG-02` AC1).
//!
//! The argv is a pure function of the OS and the path, so it is tested without a
//! window: `xdg-open` on Linux, `open` on macOS, `rundll32 url.dll,
//! FileProtocolHandler` on Windows (never `cmd /c start`, whose parser would see
//! `&`, `^`, `%` and quotes in the path). The path is always ONE argv element, so
//! a space, `&`, `%` or a leading `-` cannot become a second argument.
//!
//! The launch goes through [`OpenLauncher`] so a test injects a fake and records
//! the argv instead of opening a real editor on the user's desktop.

use std::path::{Path, PathBuf};

use crate::platform::Os;

/// One prepared "open this file" command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenCommand {
    /// Programme to run (`xdg-open`, `open`, `rundll32`).
    pub program: PathBuf,
    /// Argument vector; the path is always the last element and never split.
    pub args: Vec<String>,
}

impl OpenCommand {
    /// The argv for opening `path` with the OS default handler.
    ///
    /// `path` is passed as exactly one argument. On Linux a path that starts with
    /// `-` is passed as `./-name` (relative) or with an absolute path (which never
    /// starts with `-`), because `xdg-open` takes options first; the caller always
    /// has an absolute path here, so a leading `-` cannot occur, but the guard is
    /// kept and tested.
    pub fn for_os(os: Os, path: &Path) -> Self {
        let arg = path_arg(path);
        match os {
            Os::Linux => Self {
                program: PathBuf::from("xdg-open"),
                args: vec![arg],
            },
            Os::Macos => Self {
                program: PathBuf::from("open"),
                args: vec![arg],
            },
            Os::Windows => Self {
                program: PathBuf::from("rundll32.exe"),
                args: vec!["url.dll,FileProtocolHandler".to_owned(), arg],
            },
        }
    }
}

/// Renders `path` as one argument that cannot be read as an option.
fn path_arg(path: &Path) -> String {
    let rendered = path.display().to_string();
    // Only a bare relative file whose name begins with `-` is at risk; an
    // absolute path starts with `/` (or `C:\`), so it never does. Guard anyway.
    if rendered.starts_with('-') {
        format!("./{rendered}")
    } else {
        rendered
    }
}

/// Starts a prepared open command.
///
/// A seam so a test injects a fake. Production uses [`SystemOpenLauncher`], which
/// spawns detached with argv only, no shell, stdio to null.
pub trait OpenLauncher: Send + Sync {
    /// Launches `command`. Returns after the child has been started.
    fn launch(&self, command: &OpenCommand) -> std::io::Result<()>;
}

/// Production launcher: `std::process::Command`, detached, argv only.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemOpenLauncher;

impl OpenLauncher for SystemOpenLauncher {
    fn launch(&self, command: &OpenCommand) -> std::io::Result<()> {
        let mut cmd = std::process::Command::new(&command.program);
        for arg in &command.args {
            cmd.arg(arg);
        }
        cmd.stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        // Reap the child on a detached thread so a slow handler never blocks us
        // and no zombie is left behind. The handler exit status is irrelevant.
        let mut child = cmd.spawn()?;
        let _ = std::thread::Builder::new()
            .name("dblocal-open-wait".to_owned())
            .spawn(move || {
                let _ = child.wait();
            });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn linux_uses_xdg_open_with_one_argument() {
        let command = OpenCommand::for_os(
            Os::Linux,
            Path::new("/home/u/.dblocal/instances/a/run/my.cnf"),
        );
        assert_eq!(command.program, PathBuf::from("xdg-open"));
        assert_eq!(command.args.len(), 1);
        assert_eq!(command.args[0], "/home/u/.dblocal/instances/a/run/my.cnf");
    }

    #[test]
    fn macos_uses_open() {
        let command = OpenCommand::for_os(Os::Macos, Path::new("/x/postgresql.conf"));
        assert_eq!(command.program, PathBuf::from("open"));
        assert_eq!(command.args, vec!["/x/postgresql.conf".to_owned()]);
    }

    #[test]
    fn windows_uses_rundll32_and_never_cmd_start() {
        let command = OpenCommand::for_os(Os::Windows, Path::new("C:\\x\\my.cnf"));
        assert_eq!(command.program, PathBuf::from("rundll32.exe"));
        assert_eq!(
            command.args,
            vec![
                "url.dll,FileProtocolHandler".to_owned(),
                "C:\\x\\my.cnf".to_owned()
            ]
        );
        assert!(!command.program.to_string_lossy().contains("cmd"));
    }

    #[test]
    fn a_path_with_spaces_ampersand_and_percent_stays_one_argument() {
        let path = Path::new("/home/u/a & b/100%/my.cnf");
        for os in [Os::Linux, Os::Macos, Os::Windows] {
            let command = OpenCommand::for_os(os, path);
            // Exactly one element carries the whole path; nothing was split.
            let matches: Vec<&String> = command
                .args
                .iter()
                .filter(|arg| arg.contains("a & b"))
                .collect();
            assert_eq!(matches.len(), 1, "{os:?}: {:?}", command.args);
            assert_eq!(matches[0], "/home/u/a & b/100%/my.cnf");
        }
    }

    #[test]
    fn a_leading_dash_is_not_read_as_an_option() {
        let command = OpenCommand::for_os(Os::Linux, Path::new("-rf.conf"));
        assert_eq!(command.args.len(), 1);
        assert_eq!(command.args[0], "./-rf.conf", "must not be an option");
    }

    /// A launcher that records the argv instead of launching.
    #[derive(Default)]
    struct RecordingLauncher {
        seen: Mutex<Vec<OpenCommand>>,
        fail: bool,
    }

    impl OpenLauncher for RecordingLauncher {
        fn launch(&self, command: &OpenCommand) -> std::io::Result<()> {
            if self.fail {
                return Err(std::io::Error::other("no handler"));
            }
            self.seen.lock().expect("lock").push(command.clone());
            Ok(())
        }
    }

    #[test]
    fn the_launcher_seam_records_the_exact_argv() {
        let launcher = RecordingLauncher::default();
        let command = OpenCommand::for_os(Os::Linux, Path::new("/x/y z.conf"));
        launcher.launch(&command).expect("record");
        let seen = launcher.seen.lock().expect("lock");
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].args, vec!["/x/y z.conf".to_owned()]);
    }

    #[test]
    fn a_failing_launcher_returns_an_error_and_opens_nothing() {
        let launcher = RecordingLauncher {
            fail: true,
            ..Default::default()
        };
        let command = OpenCommand::for_os(Os::Linux, Path::new("/x/y.conf"));
        assert!(launcher.launch(&command).is_err());
        assert!(launcher.seen.lock().expect("lock").is_empty());
    }
}
