//! Redis engine adapter.
//!
//! Thin wrapper over [`crate::engine::resp`] configured for Redis 7.4 from the
//! official APT `redis-tools` package (`redis-check-rdb` multi-call binary
//! invoked as `redis-server`), with a Linux shared-library preflight check for
//! `libssl.so.3`, `libcrypto.so.3` and `libsystemd.so.0`.

use std::path::Path;
#[cfg(test)]
use std::path::PathBuf;
use std::process::Command;

use crate::consts;
use crate::engine::resp::{self, RespBrand};
use crate::engine::shlib;
use crate::engine::{AdminOps, Credential, EngineAdapter, Health, InitContext, RunContext, uri};
use crate::error::Result;
use crate::model::{EngineKey, StartFailure};
use crate::secret::SecretString;
use crate::service::UriFormat;

#[cfg(test)]
pub use crate::engine::resp::ConfigParams;
pub use crate::engine::resp::ServerIdentity;

/// Brand parameters for Redis.
pub const BRAND: RespBrand = RespBrand {
    engine: EngineKey::Redis,
    label: "redis",
    server_bin: "redis-server",
    multicall_bin: Some("redis-check-rdb"),
    config_file: "redis.conf",
    pid_file: "redis.pid",
    log_file: "redis.log",
    superuser: consts::SUPERUSER_REDIS,
};

/// Redis adapter.
#[derive(Debug, Clone, Copy, Default)]
pub struct RedisAdapter;

impl EngineAdapter for RedisAdapter {
    fn key(&self) -> EngineKey {
        EngineKey::Redis
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
        uri::render(EngineKey::Redis, ctx.port.get(), credential, format)
    }

    fn diagnose_start_failure(&self, output: &str, _exit_code: Option<i32>) -> StartFailure {
        resp::diagnose_failure_text(output)
    }

    fn admin_ops(&self) -> Option<&dyn AdminOps> {
        None
    }

    fn prepare_run(&self, ctx: &RunContext) -> Result<()> {
        prepare_run(ctx)
    }
}

/// Renders the managed `redis.conf`.
#[cfg(test)]
pub fn render_config(params: &ConfigParams<'_>) -> SecretString {
    resp::render_config(&BRAND, params)
}

/// Path of the generated `redis.conf` inside `run_dir`.
#[cfg(test)]
pub fn config_path(run_dir: &Path) -> PathBuf {
    resp::config_path(&BRAND, run_dir)
}

/// Path of `redis.pid` inside `run_dir`.
#[cfg(test)]
pub fn pidfile_path(run_dir: &Path) -> PathBuf {
    resp::pidfile_path(&BRAND, run_dir)
}

/// Path of `redis.log` inside `logs_dir`.
#[cfg(test)]
pub fn logfile_path(logs_dir: &Path) -> PathBuf {
    resp::logfile_path(&BRAND, logs_dir)
}

/// Runs the Linux shared-library preflight check and ensures private dirs exist.
pub fn prepare_run(ctx: &RunContext) -> Result<()> {
    shlib::check_redis_libs()?;
    resp::prepare_run(ctx)
}

/// Rewrites the managed `port` line in `run_dir/redis.conf` atomically.
pub fn sync_port(run_dir: &Path, port: u16) -> Result<()> {
    resp::sync_port(&BRAND, run_dir, port)
}

/// Reads and parses `<run_dir>/redis.pid`.
pub fn read_pidfile(run_dir: &Path) -> Result<ServerIdentity> {
    resp::read_pidfile(&BRAND, run_dir)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use std::fs;
    use tempfile::tempdir;

    use super::*;
    use crate::model::{AuthMode, Port};

    #[test]
    fn config_uses_redis_filenames_loopback_and_requirepass() {
        let pw = SecretString::from("rd-pass-123");
        let conf = render_config(&ConfigParams {
            port: 41301,
            data_dir: Path::new("/i/data"),
            run_dir: Path::new("/i/run"),
            logs_dir: Path::new("/i/logs"),
            password: Some(&pw),
            auth: AuthMode::Password,
        })
        .expose()
        .to_owned();
        assert!(conf.contains("bind 127.0.0.1 -::1\n"));
        assert!(conf.contains("protected-mode yes\n"));
        assert!(conf.contains("port 41301\n"));
        assert!(conf.contains("requirepass rd-pass-123\n"));
        assert!(conf.contains(&format!(
            "pidfile {}\n",
            pidfile_path(Path::new("/i/run")).display()
        )));
        assert!(conf.contains(&format!(
            "logfile {}\n",
            logfile_path(Path::new("/i/logs")).display()
        )));
        assert!(!conf.contains("0.0.0.0"));
    }

    #[test]
    fn spawn_command_uses_multicall_redis_check_rdb_when_redis_server_absent() {
        let dir = tempdir().expect("tempdir");
        let bin_dir = dir.path().join("bin");
        fs::create_dir_all(&bin_dir).expect("mkdir");
        fs::write(bin_dir.join(uri::exe_name("redis-check-rdb")), b"ELF").expect("write");

        let ctx = RunContext {
            instance_name: "cache".into(),
            runtime_dir: dir.path().to_path_buf(),
            data_dir: dir.path().join("data"),
            logs_dir: dir.path().join("logs"),
            run_dir: dir.path().join("run"),
            port: Port::new(41301).expect("port"),
            superuser: "default".into(),
            auth: AuthMode::Password,
            credential: Some(Credential::new("default", SecretString::from("secret-pw"))),
        };
        let cmd = RedisAdapter.spawn_command(&ctx);
        assert!(
            cmd.get_program()
                .to_string_lossy()
                .contains("redis-check-rdb"),
            "program was {:?}",
            cmd.get_program()
        );
        let args: Vec<String> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args, vec![config_path(&ctx.run_dir).display().to_string()]);
        assert!(!args[0].contains("secret-pw"));
    }

    #[test]
    fn spawn_command_prefers_redis_server_when_present() {
        let dir = tempdir().expect("tempdir");
        let bin_dir = dir.path().join("bin");
        fs::create_dir_all(&bin_dir).expect("mkdir");
        fs::write(bin_dir.join(uri::exe_name("redis-server")), b"ELF").expect("write");

        let ctx = RunContext {
            instance_name: "cache".into(),
            runtime_dir: dir.path().to_path_buf(),
            data_dir: dir.path().join("data"),
            logs_dir: dir.path().join("logs"),
            run_dir: dir.path().join("run"),
            port: Port::new(41301).expect("port"),
            superuser: "default".into(),
            auth: AuthMode::None,
            credential: None,
        };
        let cmd = RedisAdapter.spawn_command(&ctx);
        assert!(
            cmd.get_program().to_string_lossy().contains("redis-server"),
            "program was {:?}",
            cmd.get_program()
        );
    }

    #[test]
    fn redis_uri_and_dotenv_match_valkey_format() {
        let ctx = RunContext {
            instance_name: "cache".into(),
            runtime_dir: PathBuf::from("/opt/redis"),
            data_dir: PathBuf::from("/data"),
            logs_dir: PathBuf::from("/logs"),
            run_dir: PathBuf::from("/run"),
            port: Port::new(6379).expect("port"),
            superuser: "default".into(),
            auth: AuthMode::Password,
            credential: None,
        };
        let cred = Credential::new("default", SecretString::from("pw"));
        assert_eq!(
            RedisAdapter
                .connection_uri(&ctx, &cred, UriFormat::Uri)
                .expose(),
            "redis://default:pw@127.0.0.1:6379/0"
        );
        let env = RedisAdapter
            .connection_uri(&ctx, &cred, UriFormat::DotEnv)
            .expose()
            .to_owned();
        assert!(env.contains("REDIS_URL=redis://default:pw@127.0.0.1:6379/0\n"));
        assert!(env.contains("REDIS_PASSWORD=pw\n"));
    }
}
