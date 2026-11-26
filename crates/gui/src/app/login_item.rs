//! The "Run at login" seam (`F-AUT-01`).
//!
//! The frozen `DblService` has no login-item call — that was deliberately kept
//! out of the service surface when W4-D landed the backends — so the Settings
//! dialog talks to the W4-D core API directly. It still obeys the GUI's one
//! rule: the call happens on a background executor, never on the GPUI thread.
//!
//! [`LoginItem`] is the seam the dialog depends on, so a headless test can drive
//! every branch (registered, not registered, enable fails, status unreadable)
//! without touching the user's real login items.

use dblocal_core::autostart::{
    AutostartBackend, AutostartError, CommandRunner, LoginItem, LoginItemStatus, SystemRunner,
};
use dblocal_core::service::UserFacingError;

use std::path::PathBuf;
use std::sync::Arc;

/// Current login-item state as the dialog renders it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginStatus {
    /// Whether the item is registered after the last read or write.
    pub enabled: bool,
    /// Absolute path of the file or registry value that represents it.
    pub location: PathBuf,
}

impl From<LoginItemStatus> for LoginStatus {
    fn from(status: LoginItemStatus) -> Self {
        Self {
            enabled: status.enabled,
            location: status.location,
        }
    }
}

/// Whether the app can register a login item on this machine, and how.
pub trait LoginItemControl: Send + Sync + 'static {
    /// Reads the current state without changing it.
    fn status(&self) -> Result<LoginStatus, UserFacingError>;
    /// Registers the login item; returns the state after the write.
    fn enable(&self) -> Result<LoginStatus, UserFacingError>;
    /// Removes the login item; returns the state after the removal.
    fn disable(&self) -> Result<LoginStatus, UserFacingError>;
}

fn facing(err: AutostartError) -> UserFacingError {
    UserFacingError::from(err)
}

/// The production login item: the W4-D backend for this OS, driven through
/// `systemctl --user` / `launchctl` / `reg` on the calling thread.
pub struct HostLoginItem {
    backend: Box<dyn AutostartBackend>,
    exe: PathBuf,
}

impl HostLoginItem {
    /// Builds the backend for this host, or the typed error that explains why
    /// there is none.
    pub fn new() -> Result<Self, UserFacingError> {
        let backend =
            dblocal_core::autostart::host_backend(Arc::new(SystemRunner) as Arc<dyn CommandRunner>)
                .map_err(facing)?;
        let exe = std::env::current_exe().map_err(|err| {
            facing(AutostartError::StatusFailed {
                detail: err.to_string(),
            })
        })?;
        Ok(Self { backend, exe })
    }

    fn item(&self) -> Result<LoginItem, UserFacingError> {
        dblocal_core::autostart::login_item_for_exe(self.exe.clone()).map_err(facing)
    }
}

impl LoginItemControl for HostLoginItem {
    fn status(&self) -> Result<LoginStatus, UserFacingError> {
        self.backend.status().map(LoginStatus::from).map_err(facing)
    }

    fn enable(&self) -> Result<LoginStatus, UserFacingError> {
        let item = self.item()?;
        self.backend
            .enable(&item)
            .map(LoginStatus::from)
            .map_err(facing)
    }

    fn disable(&self) -> Result<LoginStatus, UserFacingError> {
        self.backend
            .disable()
            .map(LoginStatus::from)
            .map_err(facing)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::*;
    use dblocal_core::autostart::{AutostartResult, CommandOutput};

    /// A backend-free controller so the dialog tests never touch a real login
    /// item. Each answer is what the test asked for.
    pub struct FakeLoginItem {
        pub enabled: bool,
        pub fail_enable: bool,
        pub fail_status: bool,
    }

    impl LoginItemControl for FakeLoginItem {
        fn status(&self) -> Result<LoginStatus, UserFacingError> {
            if self.fail_status {
                return Err(UserFacingError::new("DBL-8504", "status check failed"));
            }
            Ok(LoginStatus {
                enabled: self.enabled,
                location: PathBuf::from("/tmp/fake/login-item"),
            })
        }
        fn enable(&self) -> Result<LoginStatus, UserFacingError> {
            if self.fail_enable {
                return Err(UserFacingError::new("DBL-8502", "systemctl failed"));
            }
            Ok(LoginStatus {
                enabled: true,
                location: PathBuf::from("/tmp/fake/login-item"),
            })
        }
        fn disable(&self) -> Result<LoginStatus, UserFacingError> {
            Ok(LoginStatus {
                enabled: false,
                location: PathBuf::from("/tmp/fake/login-item"),
            })
        }
    }

    /// The production seam builds or reports a typed reason, never panics.
    #[test]
    fn host_login_item_either_builds_or_reports_a_typed_error() {
        // Only the constructor is exercised: calling `status` here would touch
        // the real login item of the machine running the test.
        match HostLoginItem::new() {
            Ok(_) => {}
            Err(err) => assert!(err.code.starts_with("DBL-")),
        }
    }

    /// `LoginStatus` keeps both fields from the core value.
    #[test]
    fn login_status_carries_enabled_and_location() {
        let core_status = LoginItemStatus {
            enabled: true,
            location: PathBuf::from("/home/alice/.config/systemd/user/dblocal.service"),
        };
        let status = LoginStatus::from(core_status);
        assert!(status.enabled);
        assert!(status.location.ends_with("dblocal.service"));
    }

    /// The fake seam answers what it was told to, so the dialog test is honest.
    #[test]
    fn the_fake_seam_answers_its_configuration() {
        let off = FakeLoginItem {
            enabled: false,
            fail_enable: false,
            fail_status: false,
        };
        assert!(!off.status().expect("status").enabled);
        assert!(off.enable().expect("enable").enabled);
        assert!(!off.disable().expect("disable").enabled);

        let failing = FakeLoginItem {
            enabled: false,
            fail_enable: true,
            fail_status: true,
        };
        assert_eq!(failing.status().expect_err("status fails").code, "DBL-8504");
        assert_eq!(failing.enable().expect_err("enable fails").code, "DBL-8502");
    }

    /// A command that cannot be spawned becomes a typed autostart error, not a
    /// panic. This is the same conversion `HostLoginItem` uses.
    #[test]
    fn an_autostart_error_becomes_a_user_facing_code() {
        let err = AutostartError::CommandFailed {
            program: "systemctl",
            detail: String::from("exit 1"),
        };
        let facing = facing(err);
        assert_eq!(facing.code, dblocal_core::autostart::CODE_COMMAND_FAILED);
    }

    /// The fake runner must implement the trait the real backend takes.
    #[test]
    fn the_command_runner_trait_is_still_shaped_as_the_backend_expects() {
        struct Noop;
        impl CommandRunner for Noop {
            fn run(&self, _program: &str, _args: &[String]) -> AutostartResult<CommandOutput> {
                Ok(CommandOutput::ok())
            }
        }
        let _runner: Arc<dyn CommandRunner> = Arc::new(Noop);
    }
}
