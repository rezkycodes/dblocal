//! Windows login items: the per-user `Run` registry key.
//!
//! PRD `F-AUT-01` AC1 uses `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`,
//! which needs no elevation and exists for every user. The value is the quoted
//! absolute path of the binary plus `--autostart`, written through the
//! `reg.exe` helper so the code is testable with an injected runner and never
//! links the Windows registry API (which would not build on Linux).
//!
//! `reg.exe` accepts `/d <data>` where `<data>` is one argument; the quoting
//! rules of the command line and of `Run` values are both satisfied by
//! [`quote_windows_reg_command`].

use std::path::Path;
use std::sync::Arc;

use super::backend::{
    AutostartBackend, CommandRunner, LoginItem, LoginItemStatus, quote_windows_reg_command,
};
use super::consts;
use super::error::{AutostartError, AutostartResult};

/// Backend that writes one value under the per-user `Run` key.
pub struct WindowsBackend {
    home: std::path::PathBuf,
    runner: Arc<dyn CommandRunner>,
}

impl WindowsBackend {
    /// Builds the backend for `home` (`%USERPROFILE%`), used for status
    /// bookkeeping in tests and diagnostics.
    pub fn new(home: &Path, runner: Arc<dyn CommandRunner>) -> Self {
        Self {
            home: home.to_path_buf(),
            runner,
        }
    }

    /// The registry key path the value lives under.
    pub fn key(&self) -> &'static str {
        consts::REGISTRY_RUN_KEY
    }

    fn query(&self) -> AutostartResult<Option<String>> {
        let output = self.runner.run(
            consts::REG,
            &[
                "query".to_owned(),
                self.key().to_owned(),
                "/v".to_owned(),
                consts::REGISTRY_VALUE.to_owned(),
            ],
        )?;
        if !output.success() {
            // A non-zero exit from `reg query` means the value is absent; that
            // is a normal "not enabled" answer, not an error.
            return Ok(None);
        }
        // `reg query` prints `    DBLocal    REG_SZ    <data>` on Windows; the
        // value is everything after the last `REG_SZ`.
        let value = output
            .stdout
            .split_once("REG_SZ")
            .map(|(_, data)| data.trim().to_owned())
            .filter(|data| !data.is_empty());
        Ok(value)
    }
}

impl AutostartBackend for WindowsBackend {
    fn enable(&self, item: &LoginItem) -> AutostartResult<LoginItemStatus> {
        let command = quote_windows_reg_command(&item.exe, &item.args)?;
        let output = self.runner.run(
            consts::REG,
            &[
                "add".to_owned(),
                self.key().to_owned(),
                "/v".to_owned(),
                consts::REGISTRY_VALUE.to_owned(),
                "/t".to_owned(),
                "REG_SZ".to_owned(),
                "/d".to_owned(),
                command,
                "/f".to_owned(),
            ],
        )?;
        if !output.success() {
            return Err(AutostartError::CommandFailed {
                program: consts::REG,
                detail: if output.stderr.is_empty() {
                    format!("exit {}", output.code)
                } else {
                    output.stderr
                },
            });
        }
        Ok(LoginItemStatus {
            enabled: true,
            location: self.location(),
        })
    }

    fn disable(&self) -> AutostartResult<LoginItemStatus> {
        let output = self.runner.run(
            consts::REG,
            &[
                "delete".to_owned(),
                self.key().to_owned(),
                "/v".to_owned(),
                consts::REGISTRY_VALUE.to_owned(),
                "/f".to_owned(),
            ],
        )?;
        // `reg delete` returns non-zero when the value is already absent; that
        // is the idempotent success this method guarantees.
        let _ = output;
        Ok(LoginItemStatus {
            enabled: false,
            location: self.location(),
        })
    }

    fn status(&self) -> AutostartResult<LoginItemStatus> {
        let enabled = self.query()?.is_some();
        Ok(LoginItemStatus {
            enabled,
            location: self.location(),
        })
    }
}

impl WindowsBackend {
    fn location(&self) -> std::path::PathBuf {
        // A registry value has no filesystem path; report the key and value in
        // a single displayable form so `--json` and logs name what changed.
        self.home
            .join(format!("{}#{}", self.key(), consts::REGISTRY_VALUE))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use std::collections::HashMap;
    use std::sync::Mutex;

    use super::*;
    use crate::autostart::backend::CommandOutput;

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

        fn answer(self, key: &str, output: CommandOutput) -> Self {
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
            Ok(self
                .answers
                .lock()
                .expect("lock")
                .get(&key)
                .cloned()
                .unwrap_or_else(|| CommandOutput {
                    code: 1,
                    stdout: String::new(),
                    stderr: "absent".to_owned(),
                }))
        }
    }

    fn item(_home: &Path) -> LoginItem {
        // A Windows path is not `is_absolute()` on a Linux test host, so the
        // Windows login item is built directly: the backend only consumes it.
        LoginItem {
            exe: std::path::PathBuf::from("C:\\Program Files\\DBLocal\\dblocal.exe"),
            args: vec![consts::AUTOSTART_FLAG.to_owned()],
        }
    }

    #[test]
    fn enable_adds_a_quoted_run_value() {
        let home = tempfile::tempdir().expect("tempdir");
        let runner = Arc::new(FakeRunner::new().answer(
            "reg add HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Run /v DBLocal /t REG_SZ /d \"C:\\Program Files\\DBLocal\\dblocal.exe\" --autostart /f",
            CommandOutput::ok(),
        ));
        let backend = WindowsBackend::new(home.path(), runner.clone());
        let status = backend.enable(&item(Path::new("C:\\"))).expect("enable");
        assert!(status.enabled);
        let calls = runner.calls();
        assert_eq!(calls.len(), 1);
        let (program, args) = &calls[0];
        assert_eq!(program, "reg");
        assert_eq!(args[0], "add");
        assert_eq!(args[1], consts::REGISTRY_RUN_KEY);
        let data = &args[7];
        assert_eq!(
            data, "\"C:\\Program Files\\DBLocal\\dblocal.exe\" --autostart",
            "the path with spaces must be quoted and the flag appended"
        );
    }

    #[test]
    fn a_failing_add_is_reported_as_a_command_failure() {
        let home = tempfile::tempdir().expect("tempdir");
        let runner = Arc::new(FakeRunner::new().answer(
            "reg add HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Run /v DBLocal /t REG_SZ /d \"C:\\Program Files\\DBLocal\\dblocal.exe\" --autostart /f",
            CommandOutput {
                code: 1,
                stdout: String::new(),
                stderr: "Access is denied".to_owned(),
            },
        ));
        let backend = WindowsBackend::new(home.path(), runner);
        let err = backend
            .enable(&item(Path::new("C:\\")))
            .expect_err("must fail");
        assert_eq!(err.code(), super::super::error::CODE_COMMAND_FAILED);
    }

    #[test]
    fn status_is_enabled_when_the_value_is_present() {
        let home = tempfile::tempdir().expect("tempdir");
        let runner = Arc::new(FakeRunner::new().answer(
            "reg query HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Run /v DBLocal",
            CommandOutput {
                code: 0,
                stdout: "    DBLocal    REG_SZ    \"C:\\Program Files\\DBLocal\\dblocal.exe\" --autostart"
                    .to_owned(),
                stderr: String::new(),
            },
        ));
        let backend = WindowsBackend::new(home.path(), runner);
        let status = backend.status().expect("status");
        assert!(status.enabled);
    }

    #[test]
    fn status_is_disabled_when_the_value_is_absent() {
        let home = tempfile::tempdir().expect("tempdir");
        let backend = WindowsBackend::new(home.path(), Arc::new(FakeRunner::new()));
        let status = backend.status().expect("status");
        assert!(!status.enabled);
    }

    #[test]
    fn disable_deletes_the_value_and_is_idempotent() {
        let home = tempfile::tempdir().expect("tempdir");
        let runner = Arc::new(FakeRunner::new());
        let backend = WindowsBackend::new(home.path(), runner.clone());
        let first = backend.disable().expect("disable once");
        let second = backend.disable().expect("disable twice");
        assert!(!first.enabled);
        assert!(!second.enabled);
        let calls = runner.calls();
        assert!(calls.iter().all(|(p, a)| p == "reg" && a[0] == "delete"));
        assert_eq!(calls.len(), 2);
    }

    #[test]
    fn the_reported_location_names_the_key_and_value() {
        let home = tempfile::tempdir().expect("tempdir");
        let backend = WindowsBackend::new(home.path(), Arc::new(FakeRunner::new()));
        let location = backend.status().expect("status").location;
        let text = location.display().to_string();
        assert!(text.contains(consts::REGISTRY_RUN_KEY));
        assert!(text.contains(consts::REGISTRY_VALUE));
    }
}
