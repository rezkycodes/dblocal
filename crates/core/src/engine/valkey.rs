//! Valkey engine adapter.
//!
//! Thin wrapper over [`crate::engine::resp`]: renders `valkey.conf`, builds the
//! `valkey-server` start command, probes with `PING` over RESP, stops with
//! `SHUTDOWN`, parses the pidfile and diagnoses start failures — including
//! causes that only appear in the logfile (C-10).

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::consts;
use crate::engine::resp::{self, RespBrand};
use crate::engine::{AdminOps, Credential, EngineAdapter, Health, InitContext, RunContext, uri};
use crate::error::Result;
use crate::model::{EngineKey, StartFailure};
use crate::secret::SecretString;
use crate::service::UriFormat;

pub use crate::engine::resp::{ConfigParams, ServerIdentity};

/// Brand parameters for Valkey.
pub const BRAND: RespBrand = RespBrand {
    engine: EngineKey::Valkey,
    label: "valkey",
    server_bin: "valkey-server",
    multicall_bin: None,
    config_file: "valkey.conf",
    pid_file: "valkey.pid",
    log_file: "valkey.log",
    superuser: consts::SUPERUSER_VALKEY,
};

/// Valkey adapter.
#[derive(Debug, Clone, Copy, Default)]
pub struct ValkeyAdapter;

impl EngineAdapter for ValkeyAdapter {
    fn key(&self) -> EngineKey {
        EngineKey::Valkey
    }

    fn initialize(&self, ctx: &InitContext) -> Result<()> {
        resp::initialize(&BRAND, ctx)
    }

    fn spawn_command(&self, ctx: &RunContext) -> Command {
        resp::spawn_command(&BRAND, ctx)
    }

    fn health_check(&self, ctx: &RunContext) -> Result<Health> {
        resp::health_check(&BRAND, ctx)
    }

    fn shutdown(&self, ctx: &RunContext) -> Result<()> {
        resp::shutdown(&BRAND, ctx)
    }

    fn connection_uri(
        &self,
        ctx: &RunContext,
        credential: &Credential,
        format: UriFormat,
    ) -> SecretString {
        uri::render(EngineKey::Valkey, ctx.port.get(), credential, format)
    }

    fn diagnose_start_failure(&self, output: &str, _exit_code: Option<i32>) -> StartFailure {
        diagnose_failure_text(output)
    }

    fn admin_ops(&self) -> Option<&dyn AdminOps> {
        // Phase 2.
        None
    }
}

/// Renders the managed `valkey.conf`. Secret because it may carry `requirepass`.
pub fn render_config(params: &ConfigParams<'_>) -> SecretString {
    resp::render_config(&BRAND, params)
}

/// Path of the generated config inside the run directory.
pub fn config_path(run_dir: &Path) -> PathBuf {
    resp::config_path(&BRAND, run_dir)
}

/// Path of the server pidfile.
pub fn pidfile_path(run_dir: &Path) -> PathBuf {
    resp::pidfile_path(&BRAND, run_dir)
}

/// Path of the server logfile.
pub fn logfile_path(logs_dir: &Path) -> PathBuf {
    resp::logfile_path(&BRAND, logs_dir)
}

/// Ensures the directories the server needs exist at start time.
pub fn prepare_run(ctx: &RunContext) -> Result<()> {
    resp::prepare_run(ctx)
}

/// Rewrites the managed `port` line in `run_dir/valkey.conf`.
pub fn sync_port(run_dir: &Path, port: u16) -> Result<()> {
    resp::sync_port(&BRAND, run_dir, port)
}

/// Returns `data_dir`'s sibling directory `leaf` (`run`, `logs`).
pub fn sibling_dir(data_dir: &Path, leaf: &str) -> PathBuf {
    resp::sibling_dir(data_dir, leaf)
}

/// Parses a Valkey pidfile. Callers must still verify the executable and the
/// `/proc/<pid>` start time before signalling (C-08 discipline, D-17).
pub fn parse_pidfile(contents: &str) -> Result<ServerIdentity> {
    resp::parse_pidfile(&BRAND, contents)
}

/// Reads and parses `<run_dir>/valkey.pid`.
pub fn read_pidfile(run_dir: &Path) -> Result<ServerIdentity> {
    resp::read_pidfile(&BRAND, run_dir)
}

/// Classifies raw Valkey start output or log tail into a [`StartFailure`].
pub fn diagnose_failure_text(output: &str) -> StartFailure {
    resp::diagnose_failure_text(output)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use super::*;
    use crate::model::{AuthMode, Port};
    use crate::error::DblError;
    use tempfile::tempdir;

    fn params<'a>(password: Option<&'a SecretString>, auth: AuthMode) -> ConfigParams<'a> {
        ConfigParams {
            port: 41300,
            data_dir: Path::new("/home/u/.dblocal/instances/app/data"),
            run_dir: Path::new("/home/u/.dblocal/instances/app/run"),
            logs_dir: Path::new("/home/u/.dblocal/instances/app/logs"),
            password,
            auth,
        }
    }

    #[test]
    fn config_with_password_has_requirepass_and_loopback_bind() {
        let pw = SecretString::from("vk-pass-123");
        let p = params(Some(&pw), AuthMode::Password);
        let conf = render_config(&p).expose().to_owned();
        // The config file spells joined paths with the host separator, so the
        // expectations are built with `Path::join` too (`\` on Windows).
        let dir = p.data_dir.to_string_lossy().into_owned();
        let pidfile = pidfile_path(p.run_dir).to_string_lossy().into_owned();
        let logfile = logfile_path(p.logs_dir).to_string_lossy().into_owned();
        assert!(conf.contains("bind 127.0.0.1 -::1\n"));
        assert!(conf.contains("protected-mode yes\n"));
        assert!(conf.contains("port 41300\n"));
        assert!(conf.contains("daemonize no\n"));
        assert!(conf.contains("supervised no\n"));
        assert!(conf.contains(&format!("dir {dir}\n")));
        assert!(conf.contains(&format!("pidfile {pidfile}\n")));
        assert!(conf.contains(&format!("logfile {logfile}\n")));
        assert!(conf.contains("requirepass vk-pass-123\n"));
        assert!(conf.contains("appendonly yes\n"));
        assert!(!conf.contains("0.0.0.0"));
    }

    #[test]
    fn config_without_password_omits_requirepass() {
        let conf = render_config(&params(None, AuthMode::None))
            .expose()
            .to_owned();
        assert!(!conf.contains("requirepass"));
        // protected-mode stays on as the last line of defence (F-CRD-03 AC3).
        assert!(conf.contains("protected-mode yes"));
    }

    #[test]
    fn config_debug_and_display_are_redacted() {
        let pw = SecretString::from("super-secret-pw");
        let conf = render_config(&params(Some(&pw), AuthMode::Password));
        assert!(!format!("{conf}").contains("super-secret-pw"));
        assert!(!format!("{conf:?}").contains("super-secret-pw"));
    }

    #[test]
    fn pidfile_paths_are_under_run_and_logs() {
        assert_eq!(
            pidfile_path(Path::new("/i/run")),
            PathBuf::from("/i/run/valkey.pid")
        );
        assert_eq!(
            config_path(Path::new("/i/run")),
            PathBuf::from("/i/run/valkey.conf")
        );
        assert_eq!(
            logfile_path(Path::new("/i/logs")),
            PathBuf::from("/i/logs/valkey.log")
        );
    }

    #[test]
    fn sibling_dirs_are_under_the_instance() {
        assert_eq!(
            sibling_dir(Path::new("/i/data"), consts::DIR_RUN),
            PathBuf::from("/i/run")
        );
        assert_eq!(
            sibling_dir(Path::new("/i/data"), consts::DIR_LOGS),
            PathBuf::from("/i/logs")
        );
    }

    #[test]
    fn spawn_command_passes_only_the_config_path() {
        let ctx = RunContext {
            instance_name: "app".into(),
            runtime_dir: PathBuf::from("/opt/valkey"),
            data_dir: PathBuf::from("/data"),
            logs_dir: PathBuf::from("/logs"),
            run_dir: PathBuf::from("/run"),
            port: Port::new(41300).expect("port"),
            superuser: "default".into(),
            auth: AuthMode::Password,
            credential: Some(Credential::new("default", SecretString::from("top-secret"))),
        };
        let cmd = ValkeyAdapter.spawn_command(&ctx);
        let args: Vec<String> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args,
            vec![config_path(Path::new("/run")).display().to_string()]
        );
        assert!(
            cmd.get_program()
                .to_string_lossy()
                .contains("valkey-server")
        );
        for arg in &args {
            assert!(!arg.contains("top-secret"));
        }
        for (k, v) in cmd.get_envs() {
            assert!(!k.to_string_lossy().contains("top-secret"));
            if let Some(v) = v {
                assert!(!v.to_string_lossy().contains("top-secret"));
            }
        }
    }

    #[test]
    fn initialize_creates_dirs_and_restricted_config() {
        let dir = tempdir().expect("tempdir");
        let data = dir.path().join("instances/app/data");
        let ctx = InitContext {
            instance_name: "app".into(),
            runtime_dir: PathBuf::from("/opt/valkey"),
            data_dir: data.clone(),
            port: Port::new(41300).expect("port"),
            superuser: "default".into(),
            auth: AuthMode::Password,
            password: Some(SecretString::from("pw-123")),
        };
        ValkeyAdapter.initialize(&ctx).expect("init");
        assert!(data.is_dir());
        let logs = sibling_dir(&data, consts::DIR_LOGS);
        let run = sibling_dir(&data, consts::DIR_RUN);
        assert!(logs.is_dir());
        assert!(run.is_dir());
        let conf = fs::read_to_string(config_path(&run)).expect("config");
        assert!(conf.contains("requirepass pw-123"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(config_path(&run))
                .expect("meta")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600);
        }
    }

    #[test]
    fn pidfile_parsing() {
        assert_eq!(parse_pidfile("4242\n").expect("pid").pid, 4242);
        assert_eq!(parse_pidfile("  7  \n").expect("pid").pid, 7);
        assert!(parse_pidfile("").is_err());
        assert!(parse_pidfile("not-a-pid\n").is_err());
    }

    #[test]
    fn read_pidfile_from_disk() {
        let dir = tempdir().expect("tempdir");
        fs::write(pidfile_path(dir.path()), "999\n").expect("write");
        assert_eq!(read_pidfile(dir.path()).expect("read").pid, 999);
    }

    // ---- Table-driven diagnosis: every verbatim Valkey failure text ----

    fn cases() -> Vec<(&'static str, StartFailure)> {
        vec![
            // 6.1 port already in use
            (
                "<M>:M 20 Nov 2026 20:17:08.351 # Warning: Could not create server TCP listening socket 127.0.0.1:41501: bind: Address already in use\n<M>:M 20 Nov 2026 20:17:08.351 # Failed listening on port 41501 (tcp), aborting.\n",
                StartFailure::PortInUse,
            ),
            // 6.5 bind to unavailable address: not "port in use", so no honest
            // variant; Unknown keeps the raw tail.
            (
                "<M>:M ... # Warning: Could not create server TCP listening socket 10.255.255.1:41502: bind: Cannot assign requested address\n<M>:M ... # Failed listening on port 41502 (tcp), aborting.\n",
                StartFailure::Unknown,
            ),
            // 6.2 bad config: unknown directive
            (
                "*** FATAL CONFIG FILE ERROR (Version 9.2.0) ***\nReading the configuration file, at line 10\n>>> 'nonexistent_directive_on_purpose 1'\nBad directive or wrong number of arguments\n",
                StartFailure::Unknown,
            ),
            // 6.3 bad config: wrong value type
            (
                "*** FATAL CONFIG FILE ERROR (Version 9.2.0) ***\nReading the configuration file, at line 10\n>>> 'maxmemory not-a-number'\nargument must be a memory value\n",
                StartFailure::Unknown,
            ),
            // 6.4 bad config: port out of range
            (
                "*** FATAL CONFIG FILE ERROR (Version 9.2.0) ***\nReading the configuration file, at line 3\n>>> 'port 99999'\nargument must be between 0 and 65535 inclusive\n",
                StartFailure::Unknown,
            ),
            // 6.6 missing data directory: not a permission problem; Unknown.
            (
                "*** FATAL CONFIG FILE ERROR (Version 9.2.0) ***\nReading the configuration file, at line 7\n>>> 'dir /tmp/work/does-not-exist'\nNo such file or directory\n",
                StartFailure::Unknown,
            ),
            // 6.7 append-only dir: Permission denied is honest; "Not a
            // directory" is not, so it stays Unknown.
            (
                "<M>:M ... * Server initialized\n<M>:M ... # Can't open or create append-only dir appendonlydir: Permission denied\n",
                StartFailure::PermissionDenied,
            ),
            (
                "<M>:M ... * Server initialized\n<M>:M ... # Can't open or create append-only dir appendonlydir: Not a directory\n",
                StartFailure::Unknown,
            ),
        ]
    }

    #[test]
    fn diagnose_spike_verbatim_texts() {
        for (text, expected) in cases() {
            assert_eq!(diagnose_failure_text(text), expected, "text: {text:?}");
        }
    }

    #[test]
    fn diagnose_unknown_for_unrelated_text() {
        assert_eq!(
            diagnose_failure_text("# WARNING Memory overcommit must be enabled!\n"),
            StartFailure::Unknown
        );
    }

    #[test]
    fn sync_port_rewrites_only_port_line_atomically() {
        let dir = tempdir().expect("tempdir");
        let run = dir.path().join("run");
        fs::create_dir_all(&run).expect("run");
        let mut conf = render_config(&ConfigParams {
            port: 41000,
            data_dir: dir.path().join("data").as_path(),
            run_dir: run.as_path(),
            logs_dir: dir.path().join("logs").as_path(),
            password: None,
            auth: AuthMode::None,
        })
        .expose()
        .to_owned();
        conf.push_str("# keep-me\n");
        fs::write(config_path(&run), conf).expect("write");
        sync_port(&run, 42000).expect("sync");
        let text = fs::read_to_string(config_path(&run)).expect("read");
        assert!(text.contains("port 42000\n"));
        assert!(!text.contains("port 41000\n"));
        assert!(text.contains("# keep-me"));
        assert_eq!(text.matches("\nport ").count(), 1);
        sync_port(&run, 42000).expect("idempotent");
        let again = fs::read_to_string(config_path(&run)).expect("read");
        assert_eq!(text, again);
    }

    #[test]
    fn sync_port_refuses_missing_port_directive() {
        let dir = tempdir().expect("tempdir");
        let run = dir.path().join("run");
        fs::create_dir_all(&run).expect("run");
        fs::write(config_path(&run), "bind 127.0.0.1\n").expect("write");
        let err = sync_port(&run, 42000).unwrap_err();
        assert!(matches!(err, DblError::Engine { .. }));
    }

    #[test]
    fn prepare_run_tightens_existing_permissive_dirs() {
        let dir = tempdir().expect("tempdir");
        let data = dir.path().join("data");
        let run = dir.path().join("run");
        let logs = dir.path().join("logs");
        fs::create_dir_all(&data).expect("data");
        fs::create_dir_all(&run).expect("run");
        fs::create_dir_all(&logs).expect("logs");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for p in [&data, &run, &logs] {
                fs::set_permissions(p, fs::Permissions::from_mode(0o755)).expect("chmod");
            }
        }
        let ctx = RunContext {
            instance_name: "app".into(),
            runtime_dir: PathBuf::from("/opt/valkey"),
            data_dir: data.clone(),
            logs_dir: logs.clone(),
            run_dir: run.clone(),
            port: Port::new(41000).expect("port"),
            superuser: "default".into(),
            auth: AuthMode::None,
            credential: None,
        };
        prepare_run(&ctx).expect("prepare");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for p in [&data, &run, &logs] {
                let mode = fs::metadata(p).expect("meta").permissions().mode() & 0o777;
                assert_eq!(mode, 0o700, "{} must be 0700", p.display());
            }
        }
    }
}
