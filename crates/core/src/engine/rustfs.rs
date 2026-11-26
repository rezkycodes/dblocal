//! RustFS (S3-compatible object storage) engine adapter.
//!
//! RustFS Linux archives ship a static musl binary (`rustfs`), so no Linux
//! shared-library preflight is required. Every instance binds loopback
//! (`127.0.0.1`) exclusively for both the S3 API and the optional web console,
//! and receives per-instance random credentials via environment variables
//! (`RUSTFS_ACCESS_KEY` / `RUSTFS_SECRET_KEY`) — never via argv and never with
//! empty or default (`rustfsadmin`) credentials.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use rand::RngExt;

use crate::DblError;
use crate::consts;
use crate::engine::{AdminOps, Credential, EngineAdapter, Health, InitContext, RunContext, uri};
use crate::error::Result;
use crate::model::{AuthMode, EngineKey, StartFailure};
use crate::secret::SecretString;
use crate::service::UriFormat;

/// Default upstream credentials that DBLocal must always refuse to use.
const FORBIDDEN_DEFAULT_CREDENTIAL: &str = "rustfsadmin";
/// Timeout for a single HTTP `/health/ready` probe.
const HEALTH_PROBE_TIMEOUT: Duration = Duration::from_secs(2);
/// Uppercase alphanumeric alphabet for generated S3 access keys.
const ACCESS_KEY_ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";

/// RustFS adapter.
#[derive(Debug, Clone, Copy, Default)]
pub struct RustfsAdapter;

impl EngineAdapter for RustfsAdapter {
    fn key(&self) -> EngineKey {
        EngineKey::Rustfs
    }

    fn initialize(&self, ctx: &InitContext) -> Result<()> {
        if ctx.auth != AuthMode::Password {
            return Err(DblError::InvalidInput {
                field: "auth",
                reason:
                    "RustFS requires generated credentials and cannot run without authentication"
                        .to_owned(),
            });
        }
        let secret = ctx
            .password
            .as_ref()
            .ok_or_else(|| DblError::InvalidInput {
                field: "password",
                reason: "RustFS requires a non-empty secret key".to_owned(),
            })?;
        validate_credentials(&ctx.superuser, secret.expose())?;
        crate::fsutil::ensure_private_dir(&ctx.data_dir)
            .map_err(|source| DblError::Io { source })?;
        Ok(())
    }

    fn spawn_command(&self, ctx: &RunContext) -> Command {
        let bin = resolve_binary(&ctx.runtime_dir);
        let mut cmd = Command::new(bin);
        cmd.arg("--address")
            .arg(format!("127.0.0.1:{}", ctx.port.get()));
        if let Some(console_port) = ctx.extra_port {
            cmd.arg("--console-enable")
                .arg("--console-address")
                .arg(format!("127.0.0.1:{}", console_port.get()));
        }
        cmd.arg(&ctx.data_dir);

        if let Some(cred) = &ctx.credential {
            cmd.env("RUSTFS_ACCESS_KEY", &cred.username);
            cmd.env("RUSTFS_SECRET_KEY", cred.password.expose());
        }
        cmd
    }

    fn health_check(&self, ctx: &RunContext) -> Result<Health> {
        probe_health_ready(ctx.port.get())
    }

    fn shutdown(&self, _ctx: &RunContext) -> Result<()> {
        // RustFS has no local protocol shutdown RPC; returning an error lets
        // `instance::stop` immediately deliver a verified graceful signal
        // (SIGTERM on Unix after PID + exe + start-time checks, terminate on
        // Windows) without idling for the full grace period first.
        Err(DblError::Engine {
            reason: "rustfs shuts down via verified process signal".to_owned(),
        })
    }

    fn connection_uri(
        &self,
        ctx: &RunContext,
        credential: &Credential,
        format: UriFormat,
    ) -> SecretString {
        match format {
            UriFormat::Uri => uri::build_uri(EngineKey::Rustfs, ctx.port.get(), credential),
            UriFormat::DotEnv => {
                let base = uri::build_dotenv(EngineKey::Rustfs, ctx.port.get(), credential);
                match ctx.extra_port {
                    Some(console_port) => SecretString::new(format!(
                        "{}RUSTFS_CONSOLE_URL=http://127.0.0.1:{}\n",
                        base.expose(),
                        console_port.get()
                    )),
                    None => base,
                }
            }
        }
    }

    fn diagnose_start_failure(&self, output: &str, _exit_code: Option<i32>) -> StartFailure {
        let lower = output.to_ascii_lowercase();
        if lower.contains("address already in use") || lower.contains("bind") {
            StartFailure::PortInUse
        } else if lower.contains("permission denied") {
            StartFailure::PermissionDenied
        } else {
            StartFailure::Unknown
        }
    }

    fn admin_ops(&self) -> Option<&dyn AdminOps> {
        None
    }

    fn prepare_run(&self, ctx: &RunContext) -> Result<()> {
        prepare_run(ctx)
    }
}

/// Prepares runtime directories and validates credentials before starting RustFS.
///
/// No Linux shared-library preflight is needed because DBLocal ships the static
/// musl build (`rustfs-linux-*-musl`) with zero dynamic library dependencies.
pub fn prepare_run(ctx: &RunContext) -> Result<()> {
    let cred = ctx
        .credential
        .as_ref()
        .ok_or_else(|| DblError::InvalidInput {
            field: "credential",
            reason: "RustFS refuses to start without per-instance credentials".to_owned(),
        })?;
    validate_credentials(&cred.username, cred.password.expose())?;
    for dir in [&ctx.data_dir, &ctx.logs_dir, &ctx.run_dir] {
        crate::fsutil::ensure_private_dir(dir).map_err(|source| DblError::Io { source })?;
    }
    Ok(())
}

/// Generates a random 20-character uppercase alphanumeric S3 access key.
pub fn generate_access_key() -> String {
    let mut rng = rand::rng();
    let mut out = String::with_capacity(consts::ACCESS_KEY_LEN);
    out.push_str("DBL");
    for _ in 3..consts::ACCESS_KEY_LEN {
        let idx = rng.random_range(0..ACCESS_KEY_ALPHABET.len());
        out.push(ACCESS_KEY_ALPHABET[idx] as char);
    }
    out
}

/// Rejects empty or default (`rustfsadmin`) credentials.
pub fn validate_credentials(access_key: &str, secret_key: &str) -> Result<()> {
    let ak = access_key.trim();
    let sk = secret_key.trim();
    if ak.is_empty() || sk.is_empty() {
        return Err(DblError::InvalidInput {
            field: "credential",
            reason: "RustFS access key and secret key must not be empty".to_owned(),
        });
    }
    if ak.eq_ignore_ascii_case(FORBIDDEN_DEFAULT_CREDENTIAL)
        || sk.eq_ignore_ascii_case(FORBIDDEN_DEFAULT_CREDENTIAL)
    {
        return Err(DblError::InvalidInput {
            field: "credential",
            reason: "RustFS refuses to start with default rustfsadmin credentials".to_owned(),
        });
    }
    if sk.len() < consts::PASSWORD_LEN {
        return Err(DblError::InvalidInput {
            field: "credential",
            reason: format!(
                "RustFS secret key must be at least {} characters",
                consts::PASSWORD_LEN
            ),
        });
    }
    Ok(())
}

fn resolve_binary(runtime_dir: &std::path::Path) -> PathBuf {
    let root_bin = runtime_dir.join(uri::exe_name("rustfs"));
    if root_bin.exists() {
        return root_bin;
    }
    let nested = runtime_dir.join("bin").join(uri::exe_name("rustfs"));
    if nested.exists() {
        return nested;
    }
    root_bin
}

/// Performs a synchronous HTTP `GET /health/ready` check against `127.0.0.1:<port>`.
pub fn probe_health_ready(port: u16) -> Result<Health> {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let mut stream = match TcpStream::connect_timeout(&addr, HEALTH_PROBE_TIMEOUT) {
        Ok(stream) => stream,
        Err(err) => return Ok(Health::NotReady(err.to_string())),
    };
    let _ = stream.set_read_timeout(Some(HEALTH_PROBE_TIMEOUT));
    let _ = stream.set_write_timeout(Some(HEALTH_PROBE_TIMEOUT));

    let request = format!(
        "GET /health/ready HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
    );
    if let Err(err) = stream.write_all(request.as_bytes()) {
        return Ok(Health::NotReady(err.to_string()));
    }

    let mut buf = [0u8; 256];
    let n = match stream.read(&mut buf) {
        Ok(n) => n,
        Err(err) => return Ok(Health::NotReady(err.to_string())),
    };
    let head = String::from_utf8_lossy(&buf[..n]);
    let status_line = head.lines().next().unwrap_or("");
    let code = status_line.split_whitespace().nth(1);
    if code == Some("200") {
        Ok(Health::Ready)
    } else {
        Ok(Health::NotReady(format!(
            "unexpected /health/ready status: {status_line}"
        )))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use std::io::{Read, Write};
    use std::net::TcpListener;
    use tempfile::tempdir;

    use super::*;
    use crate::model::Port;

    fn sample_run_ctx(dir: &std::path::Path, extra_port: Option<u16>) -> RunContext {
        RunContext {
            instance_name: "s3-local".into(),
            runtime_dir: dir.join("rt"),
            data_dir: dir.join("data"),
            logs_dir: dir.join("logs"),
            run_dir: dir.join("run"),
            port: Port::new(49000).expect("port"),
            extra_port: extra_port.map(|p| Port::new(p).expect("extra_port")),
            superuser: "DBLTESTKEY1234567890".into(),
            auth: AuthMode::Password,
            credential: Some(Credential::new(
                "DBLTESTKEY1234567890",
                SecretString::from("super-secret-rustfs-key-24plus"),
            )),
        }
    }

    #[test]
    fn spawn_command_uses_loopback_and_passes_credentials_only_via_env() {
        let dir = tempdir().expect("tempdir");
        let ctx = sample_run_ctx(dir.path(), Some(49001));
        let cmd = RustfsAdapter.spawn_command(&ctx);
        let args: Vec<String> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args,
            vec![
                "--address",
                "127.0.0.1:49000",
                "--console-enable",
                "--console-address",
                "127.0.0.1:49001",
                &ctx.data_dir.display().to_string(),
            ]
        );
        // Secrets must never appear in argv.
        assert!(!args.iter().any(|a| a.contains("super-secret")));
        assert!(!args.iter().any(|a| a.contains("DBLTESTKEY")));
        assert!(!args.iter().any(|a| a.contains("0.0.0.0")));

        let envs: std::collections::BTreeMap<String, String> = cmd
            .get_envs()
            .filter_map(|(k, v)| {
                Some((
                    k.to_string_lossy().into_owned(),
                    v?.to_string_lossy().into_owned(),
                ))
            })
            .collect();
        assert_eq!(
            envs.get("RUSTFS_ACCESS_KEY").map(String::as_str),
            Some("DBLTESTKEY1234567890")
        );
        assert_eq!(
            envs.get("RUSTFS_SECRET_KEY").map(String::as_str),
            Some("super-secret-rustfs-key-24plus")
        );
    }

    #[test]
    fn spawn_command_omits_console_flags_when_console_is_off() {
        let dir = tempdir().expect("tempdir");
        let ctx = sample_run_ctx(dir.path(), None);
        let cmd = RustfsAdapter.spawn_command(&ctx);
        let args: Vec<String> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert!(!args.iter().any(|a| a.contains("console")));
        assert_eq!(
            args,
            vec![
                "--address",
                "127.0.0.1:49000",
                &ctx.data_dir.display().to_string(),
            ]
        );
    }

    #[test]
    fn refuses_empty_or_default_rustfsadmin_credentials() {
        let dir = tempdir().expect("tempdir");
        let mut ctx = sample_run_ctx(dir.path(), None);
        assert!(prepare_run(&ctx).is_ok());

        ctx.credential = None;
        assert!(prepare_run(&ctx).is_err());

        ctx.credential = Some(Credential::new(
            "rustfsadmin",
            SecretString::from("super-secret-rustfs-key-24plus"),
        ));
        assert!(prepare_run(&ctx).is_err());

        ctx.credential = Some(Credential::new(
            "DBLTESTKEY1234567890",
            SecretString::from("rustfsadmin"),
        ));
        assert!(prepare_run(&ctx).is_err());

        ctx.credential = Some(Credential::new(
            "DBLTESTKEY1234567890",
            SecretString::from("short"),
        ));
        assert!(prepare_run(&ctx).is_err());
    }

    #[test]
    fn health_check_reads_http_200_from_health_ready() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut req = [0u8; 256];
            let n = stream.read(&mut req).expect("read");
            let text = String::from_utf8_lossy(&req[..n]);
            assert!(text.starts_with("GET /health/ready HTTP/1.1"), "{text}");
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                .expect("write");
        });
        assert_eq!(probe_health_ready(port).expect("probe"), Health::Ready);
        handle.join().expect("join");
    }

    #[test]
    fn health_check_treats_non_200_as_not_ready() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut req = [0u8; 256];
            let _ = stream.read(&mut req);
            stream
                .write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\n\r\n")
                .expect("write");
        });
        assert!(matches!(
            probe_health_ready(port).expect("probe"),
            Health::NotReady(_)
        ));
        handle.join().expect("join");
    }

    #[test]
    fn generate_access_key_is_random_and_valid() {
        let k1 = generate_access_key();
        let k2 = generate_access_key();
        assert_eq!(k1.len(), consts::ACCESS_KEY_LEN);
        assert_ne!(k1, k2);
        assert!(k1.starts_with("DBL"));
        assert!(
            k1.bytes()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
        );
    }

    #[test]
    fn connection_uri_and_dotenv_match_s3_contract() {
        let dir = tempdir().expect("tempdir");
        let ctx = sample_run_ctx(dir.path(), Some(49001));
        let cred = ctx.credential.as_ref().expect("cred");
        assert_eq!(
            RustfsAdapter
                .connection_uri(&ctx, cred, UriFormat::Uri)
                .expose(),
            "http://127.0.0.1:49000"
        );
        let env = RustfsAdapter
            .connection_uri(&ctx, cred, UriFormat::DotEnv)
            .expose()
            .to_owned();
        assert!(env.contains("AWS_ENDPOINT_URL=http://127.0.0.1:49000\n"));
        assert!(env.contains("AWS_ACCESS_KEY_ID=DBLTESTKEY1234567890\n"));
        assert!(env.contains("AWS_SECRET_ACCESS_KEY=super-secret-rustfs-key-24plus\n"));
        assert!(env.contains("RUSTFS_CONSOLE_URL=http://127.0.0.1:49001\n"));
    }
}
