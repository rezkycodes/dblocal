//! Launch execution through an injected spawner.

use std::path::PathBuf;
use std::process::Command;

use crate::error::{DblError, Result};

use super::plan::{LaunchEnv, LaunchPlan, TempFile};

/// Executes a prepared launch plan.
///
/// Implementations must not log secret env values or temp file contents.
pub trait Spawner: Send + Sync {
    /// Spawns the plan. Returns after the child has been started.
    fn spawn(&self, plan: &LaunchPlan) -> Result<()>;
}

/// Spawner that uses `std::process::Command`.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemSpawner;

impl Spawner for SystemSpawner {
    fn spawn(&self, plan: &LaunchPlan) -> Result<()> {
        let script_path = materialize_temp_file(plan.temp_file())?;
        let mut cmd = Command::new(plan.program());
        cmd.args(plan.argv());
        for env in plan.env() {
            cmd.env(env.name(), env.value().expose());
        }
        if let Some(path) = script_path.as_ref() {
            cmd.env("DBLOCAL_LAUNCHER_SCRIPT", path);
        }
        cmd.stdin(std::process::Stdio::null());
        cmd.stdout(std::process::Stdio::null());
        cmd.stderr(std::process::Stdio::null());
        let mut child = cmd.spawn().map_err(|err| {
            if err.kind() == std::io::ErrorKind::NotFound {
                DblError::NotFound {
                    kind: "client",
                    name: plan.program().display().to_string(),
                }
            } else {
                DblError::Io { source: err }
            }
        })?;
        // We do not wait; launcher scripts remove themselves. Reap in a thread
        // to avoid zombies.
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        Ok(())
    }
}

/// Runs a plan through the provided spawner.
pub fn run(plan: &LaunchPlan, spawner: &dyn Spawner) -> Result<()> {
    spawner.spawn(plan)
}

fn materialize_temp_file(temp: Option<&TempFile>) -> Result<Option<PathBuf>> {
    let Some(temp) = temp else {
        return Ok(None);
    };
    let base = std::env::temp_dir().join("dblocal-open-client");
    std::fs::create_dir_all(&base)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&base, std::fs::Permissions::from_mode(0o700))?;
    }
    let path = base.join(temp.name_hint());
    let mut options = std::fs::OpenOptions::new();
    options.create(true).write(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(temp.mode());
    }
    use std::io::Write;
    let mut file = options.open(&path)?;
    file.write_all(temp.contents().expose().as_bytes())?;
    file.sync_all()?;
    Ok(Some(path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secret::SecretString;

    struct NoopSpawner;

    impl Spawner for NoopSpawner {
        fn spawn(&self, _plan: &LaunchPlan) -> Result<()> {
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
}
