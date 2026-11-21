//! Launch execution through an injected spawner.

use std::path::PathBuf;
use std::process::Command;

use super::error::{ClientError, ClientResult};

use super::plan::LaunchPlan;
use super::terminal::{materialize_launcher, substitute_script_placeholder};

/// Executes a prepared launch plan.
///
/// Implementations must not log secret env values or temp file contents.
pub trait Spawner: Send + Sync {
    /// Spawns the plan. Returns after the child has been started.
    fn spawn(&self, plan: &LaunchPlan) -> ClientResult<()>;
}

/// Spawner that uses `std::process::Command`.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemSpawner;

impl Spawner for SystemSpawner {
    fn spawn(&self, plan: &LaunchPlan) -> ClientResult<()> {
        if let Some(required) = plan.required_path()
            && !required.exists()
        {
            return Err(ClientError::ClientNotFound {
                name: required.display().to_string(),
            });
        }
        let script_path = materialize_temp_file(plan)?;
        let mut cmd = Command::new(plan.program());
        for arg in plan.argv() {
            if let Some(ref sp) = script_path {
                cmd.arg(substitute_script_placeholder(arg, sp));
            } else {
                cmd.arg(arg);
            }
        }
        for env in plan.env() {
            cmd.env(env.name(), env.value().expose());
        }
        cmd.stdin(std::process::Stdio::null());
        cmd.stdout(std::process::Stdio::null());
        cmd.stderr(std::process::Stdio::null());
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(err) => {
                if let Some(ref sp) = script_path {
                    let _ = std::fs::remove_file(sp);
                }
                return Err(if err.kind() == std::io::ErrorKind::NotFound {
                    ClientError::ClientNotFound {
                        name: plan.program().display().to_string(),
                    }
                } else {
                    ClientError::LaunchFailed {
                        reason: err.kind().to_string(),
                    }
                });
            }
        };
        std::thread::Builder::new()
            .name("dblocal-client-wait".to_owned())
            .spawn(move || {
                let _ = child.wait();
            })
            .map_err(|e| ClientError::LaunchFailed {
                reason: e.kind().to_string(),
            })?;
        Ok(())
    }
}

/// Runs a plan through the provided spawner.
pub fn run(plan: &LaunchPlan, spawner: &dyn Spawner) -> ClientResult<()> {
    spawner.spawn(plan)
}

fn materialize_temp_file(plan: &LaunchPlan) -> ClientResult<Option<PathBuf>> {
    let Some(temp) = plan.temp_file() else {
        return Ok(None);
    };
    let os = if cfg!(target_os = "macos") {
        super::plan::OsKind::Macos
    } else if cfg!(target_os = "windows") {
        super::plan::OsKind::Windows
    } else {
        super::plan::OsKind::Linux
    };
    materialize_launcher(temp, os).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clients::{ClientError, LaunchEnv, LaunchPlan};
    use crate::secret::SecretString;

    struct NoopSpawner;

    impl Spawner for NoopSpawner {
        fn spawn(&self, _plan: &LaunchPlan) -> crate::clients::ClientResult<()> {
            Ok(())
        }
    }

    #[test]
    fn run_delegates_to_spawner() {
        let plan = LaunchPlan::new(
            PathBuf::from("true"),
            vec![],
            vec![LaunchEnv::new("A", SecretString::from("b"))],
            None,
            None,
            None,
        );
        run(&plan, &NoopSpawner).expect("noop");
    }

    #[test]
    fn missing_required_client_returns_not_found() {
        let plan = LaunchPlan::new("missing-client", vec![], vec![], None, None, None)
            .with_required_path("/tmp/definitely-missing-dblocal-client");
        let err = SystemSpawner.spawn(&plan).unwrap_err();
        assert!(matches!(err, ClientError::ClientNotFound { .. }), "{err:?}");
    }
}
