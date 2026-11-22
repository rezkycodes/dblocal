//! macOS login items: a LaunchAgent plist in `~/Library/LaunchAgents/`.
//!
//! PRD `F-AUT-01` AC1 uses a LaunchAgent (user-level, no sudo, nothing outside
//! the user's own `~/Library`). The agent runs the same binary plus
//! `--autostart`, and `launchctl bootstrap`/`bootout` register it in the GUI
//! domain of the current user.
//!
//! Nothing mac-specific is used here, so the module compiles and is unit-tested
//! on Linux against a temp home and a fake runner; only running it needs macOS.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::backend::{AutostartBackend, CommandRunner, LoginItem, LoginItemStatus, xml_escape};
use super::consts;
use super::error::{AutostartError, AutostartResult};

/// `~/Library/LaunchAgents/<label>.plist`.
fn plist_file_path(home: &Path) -> PathBuf {
    home.join("Library")
        .join("LaunchAgents")
        .join(format!("{}.plist", consts::MACOS_LABEL))
}

/// LaunchAgent backend.
pub struct MacosBackend {
    plist_path: PathBuf,
    uid: u32,
    runner: Arc<dyn CommandRunner>,
}

impl MacosBackend {
    /// Builds the backend for `home`, using the current user's uid.
    pub fn new(home: &Path, runner: Arc<dyn CommandRunner>) -> Self {
        Self::with_uid(home, current_uid(), runner)
    }

    /// Builds the backend with an explicit uid, so a test can assert the exact
    /// `launchctl` domain without depending on the host user.
    pub fn with_uid(home: &Path, uid: u32, runner: Arc<dyn CommandRunner>) -> Self {
        Self {
            plist_path: plist_file_path(home),
            uid,
            runner,
        }
    }

    /// Path of the plist this backend writes.
    pub fn plist_path(&self) -> &Path {
        &self.plist_path
    }

    fn domain(&self) -> String {
        format!("gui/{}", self.uid)
    }

    fn agent_target(&self) -> String {
        format!("{}/{}", self.domain(), consts::MACOS_LABEL)
    }
}

impl AutostartBackend for MacosBackend {
    fn enable(&self, item: &LoginItem) -> AutostartResult<LoginItemStatus> {
        let body = plist_body(item)?;
        if let Some(parent) = self.plist_path.parent() {
            fs::create_dir_all(parent).map_err(|_| AutostartError::WriteFailed {
                path: self.plist_path.display().to_string(),
            })?;
        }
        fs::write(&self.plist_path, &body).map_err(|_| AutostartError::WriteFailed {
            path: self.plist_path.display().to_string(),
        })?;
        // Register without failing the whole call if the agent is already
        // bootstrapped (`bootstrap` then returns "service already loaded"),
        // which is the normal idempotent re-enable case.
        let _ = self.runner.run(
            consts::LAUNCHCTL,
            &[
                "bootstrap".to_owned(),
                self.domain(),
                self.plist_path.display().to_string(),
            ],
        );
        let _ = self.runner.run(
            consts::LAUNCHCTL,
            &["kickstart".to_owned(), "-k".to_owned(), self.agent_target()],
        );
        Ok(LoginItemStatus {
            enabled: true,
            location: self.plist_path.clone(),
        })
    }

    fn disable(&self) -> AutostartResult<LoginItemStatus> {
        let plist = self.plist_path.clone();
        if plist.exists() {
            // `bootout` first, so launchd stops caring about the file before it
            // disappears; a stale registration would otherwise keep trying to
            // launch a binary that may be gone.
            let _ = self.runner.run(
                consts::LAUNCHCTL,
                &["bootout".to_owned(), self.agent_target()],
            );
            fs::remove_file(&plist).map_err(|_| AutostartError::WriteFailed {
                path: plist.display().to_string(),
            })?;
        }
        Ok(LoginItemStatus {
            enabled: false,
            location: plist,
        })
    }

    fn status(&self) -> AutostartResult<LoginItemStatus> {
        Ok(LoginItemStatus {
            enabled: self.plist_path.exists(),
            location: self.plist_path.clone(),
        })
    }
}

fn plist_body(item: &LoginItem) -> AutostartResult<String> {
    let exe = item.exe.to_str().ok_or(AutostartError::InvalidExe {
        reason: "not utf-8",
    })?;
    let mut args = String::new();
    for arg in &item.args {
        args.push_str("        <string>");
        args.push_str(&xml_escape(arg));
        args.push_str("</string>\n");
    }
    Ok(format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
         \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n\
         <dict>\n\
         \x20   <key>Label</key>\n\
         \x20   <string>{label}</string>\n\
         \x20   <key>ProgramArguments</key>\n\
         \x20   <array>\n\
         \x20       <string>{exe}</string>\n\
         {args}\
         \x20   </array>\n\
         \x20   <key>RunAtLoad</key>\n\
         \x20   <true/>\n\
         </dict>\n\
         </plist>\n",
        label = consts::MACOS_LABEL,
        exe = xml_escape(exe),
        args = args,
    ))
}

#[cfg(unix)]
fn current_uid() -> u32 {
    nix::unistd::getuid().as_raw()
}

#[cfg(not(unix))]
fn current_uid() -> u32 {
    // Not reached on a real macOS build; keeps the module compiling on a
    // non-unix cross-check target.
    501
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use std::collections::HashMap;
    use std::sync::Mutex;

    use super::*;
    use crate::autostart::backend::CommandOutput;
    use tempfile::tempdir;

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
    }

    impl CommandRunner for FakeRunner {
        fn run(&self, program: &str, args: &[String]) -> AutostartResult<CommandOutput> {
            self.calls
                .lock()
                .expect("lock")
                .push((program.to_owned(), args.to_vec()));
            let key = format!("{program} {}", args.join(" "));
            Ok(self
                .answers
                .lock()
                .expect("lock")
                .get(&key)
                .cloned()
                .unwrap_or_else(CommandOutput::ok))
        }
    }

    fn item(home: &Path) -> LoginItem {
        LoginItem::for_exe(home.join("Applications").join("DB Local.app")).expect("item")
    }

    #[test]
    fn enable_writes_a_plist_with_the_escaped_exe_and_flag() {
        let home = tempdir().expect("tempdir");
        let runner = Arc::new(FakeRunner::new());
        let backend = MacosBackend::with_uid(home.path(), 501, runner.clone());
        let status = backend.enable(&item(home.path())).expect("enable");
        assert!(status.enabled);
        assert_eq!(status.location, backend.plist_path());
        let body = fs::read_to_string(backend.plist_path()).expect("plist");
        assert!(body.contains("<key>Label</key>"));
        assert!(body.contains(consts::MACOS_LABEL));
        assert!(body.contains("--autostart"));
        assert!(body.contains("DB Local.app"), "path kept: {body}");
        assert!(body.contains("<key>RunAtLoad</key>"));
        assert!(body.ends_with("</plist>\n"));
        let calls = runner.calls.lock().expect("lock").clone();
        assert!(
            calls.iter().any(|(p, a)| p == "launchctl"
                && a.first().map(String::as_str) == Some("bootstrap")
                && a.get(1).map(String::as_str) == Some("gui/501")),
            "bootstrap domain wrong: {calls:?}"
        );
    }

    #[test]
    fn an_ampersand_in_the_path_is_xml_escaped() {
        let home = tempdir().expect("tempdir");
        let backend = MacosBackend::with_uid(home.path(), 501, Arc::new(FakeRunner::new()));
        let exe = home.path().join("A & B").join("dblocal");
        backend
            .enable(&LoginItem::for_exe(exe).expect("item"))
            .expect("enable");
        let body = fs::read_to_string(backend.plist_path()).expect("plist");
        assert!(body.contains("A &amp; B"), "must be escaped: {body}");
        assert!(!body.contains("A & B"));
    }

    #[test]
    fn disable_boots_out_and_removes_the_plist() {
        let home = tempdir().expect("tempdir");
        let runner = Arc::new(FakeRunner::new());
        let backend = MacosBackend::with_uid(home.path(), 501, runner.clone());
        backend.enable(&item(home.path())).expect("enable");
        let status = backend.disable().expect("disable");
        assert!(!status.enabled);
        assert!(!backend.plist_path().exists());
        let calls = runner.calls.lock().expect("lock").clone();
        assert!(
            calls
                .iter()
                .any(|(_, a)| a.first().map(String::as_str) == Some("bootout")),
            "bootout never called: {calls:?}"
        );
    }

    #[test]
    fn disable_is_idempotent_and_status_reflects_absence() {
        let home = tempdir().expect("tempdir");
        let backend = MacosBackend::with_uid(home.path(), 501, Arc::new(FakeRunner::new()));
        assert!(!backend.status().expect("status").enabled);
        backend.disable().expect("disable once");
        backend.disable().expect("disable twice");
        assert!(!backend.status().expect("status").enabled);
    }

    #[test]
    fn the_domain_uses_the_injected_uid() {
        let home = tempdir().expect("tempdir");
        let backend = MacosBackend::with_uid(home.path(), 1000, Arc::new(FakeRunner::new()));
        assert_eq!(backend.domain(), "gui/1000");
        assert_eq!(
            backend.agent_target(),
            format!("gui/1000/{}", consts::MACOS_LABEL)
        );
    }
}
