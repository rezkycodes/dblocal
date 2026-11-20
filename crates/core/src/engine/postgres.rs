//! PostgreSQL engine adapter.
//!
//! Implements initdb bootstrap, config rendering, `pg_ctl` start/stop command
//! construction, `postmaster.pid` identity parsing, protocol health checks and
//! start-failure diagnosis from the spike findings (C-06, C-07, C-08, C-09).

use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::consts;
use crate::engine::uri::exe_name;
use crate::engine::{AdminOps, Credential, EngineAdapter, Health, InitContext, RunContext, uri};
use crate::error::{DblError, Result};
use crate::model::{AuthMode, EngineKey, StartFailure};
use crate::secret::SecretString;
use crate::service::UriFormat;

/// Maximum length of an AF_UNIX socket path on Linux (`sun_path`, 107 bytes).
/// The listening socket is `<dir>/.s.PGSQL.<port>` (C-09).
const UNIX_SOCKET_PATH_LIMIT: usize = 107;

/// PostgreSQL adapter.
#[derive(Debug, Clone, Copy, Default)]
pub struct PostgresAdapter;

/// Validates and creates the unix socket directory for a start.
///
/// `spawn_command` cannot fail, so the service calls this before spawning: it
/// re-checks `<run_dir>/sock/.s.PGSQL.<port>` against the 107-byte limit at
/// start time as well as create time (C-09), and makes sure the directory the
/// server writes its socket lock into exists and is writable.
pub fn prepare_run(ctx: &RunContext) -> Result<PathBuf> {
    let dir = socket_dir_for_run(&ctx.run_dir);
    validate_socket_path_len(&dir, ctx.port.get())?;
    ensure_socket_dir(&dir)?;
    Ok(dir)
}

impl EngineAdapter for PostgresAdapter {
    fn key(&self) -> EngineKey {
        EngineKey::Postgresql
    }

    fn initialize(&self, ctx: &InitContext) -> Result<()> {
        let runtime_bin = ctx.runtime_dir.join("bin");
        let initdb = runtime_bin.join(exe_name("initdb"));
        let superuser = effective_superuser(ctx);
        let auth = auth_args(ctx.auth);
        let socket_dir = socket_dir_for_init(ctx)?;
        ensure_socket_dir(&socket_dir)?;
        let locale = select_locale()?;
        tracing::debug!(
            instance = %ctx.instance_name,
            locale = %locale,
            socket_dir = %socket_dir.display(),
            "postgres initdb"
        );

        if let Some(parent) = ctx.data_dir.parent() {
            fs::create_dir_all(parent).map_err(|e| DblError::Engine {
                reason: format!("cannot create data directory parent: {e}"),
            })?;
        }

        let pwfile_path = if ctx.with_password() {
            Some(write_pwfile(&ctx.data_dir, ctx.password.as_ref())?)
        } else {
            None
        };

        let mut cmd = Command::new(&initdb);
        cmd.arg("-D")
            .arg(&ctx.data_dir)
            .arg("-U")
            .arg(&superuser)
            .arg("--encoding=UTF8")
            .arg(format!("--locale={locale}"))
            .arg("--auth-local")
            .arg(auth.local)
            .arg("--auth-host")
            .arg(auth.host);
        if let Some(pwfile) = pwfile_path.as_ref() {
            cmd.arg("--pwfile").arg(pwfile);
        }
        strip_password_env(&mut cmd);

        let outcome = cmd.output();
        let init_result = match outcome {
            Ok(out) if out.status.success() => configure_after_init(ctx, &socket_dir),
            Ok(out) => Err(initdb_failed(out.stdout, out.stderr, out.status.code())),
            Err(err) => Err(DblError::Engine {
                reason: format!("failed to execute initdb: {err}"),
            }),
        };

        if let Some(pwfile) = pwfile_path.as_ref() {
            let _ = fs::remove_file(pwfile);
        }
        if let Err(err) = init_result {
            let _ = fs::remove_dir_all(&ctx.data_dir);
            return Err(err);
        }
        Ok(())
    }

    fn spawn_command(&self, ctx: &RunContext) -> Command {
        let pg_ctl = ctx.runtime_dir.join("bin").join(exe_name("pg_ctl"));
        let mut cmd = Command::new(pg_ctl);
        cmd.arg("-D")
            .arg(&ctx.data_dir)
            .arg("-l")
            .arg(log_path(ctx))
            .arg("-w")
            .arg("-t")
            .arg(consts::START_TIMEOUT_SECS.to_string())
            .arg("start");
        strip_password_env(&mut cmd);
        cmd.stdin(Stdio::null());
        cmd
    }

    fn health_check(&self, ctx: &RunContext) -> Result<Health> {
        // `pg_isready` alone is not enough (C-06). Run a real protocol query so
        // the password never leaves the process.
        let mut cfg = tokio_postgres::Config::new();
        cfg.host("127.0.0.1")
            .port(ctx.port.get())
            .dbname(consts::DEFAULT_DATABASE)
            .user(&ctx.superuser)
            .connect_timeout(std::time::Duration::from_secs(5));
        if ctx.auth == AuthMode::Password
            && let Some(cred) = ctx.credential.as_ref()
        {
            cfg.password(cred.password.expose().as_bytes());
        }
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| DblError::Engine {
                reason: format!("failed to build runtime for health check: {e}"),
            })?;
        rt.block_on(async {
            match cfg.connect(tokio_postgres::NoTls).await {
                Ok((client, connection)) => {
                    tokio::spawn(async move {
                        let _ = connection.await;
                    });
                    client
                        .simple_query("SELECT 1")
                        .await
                        .map_err(|e| DblError::Engine {
                            reason: format!("health query failed: {e}"),
                        })?;
                    Ok(Health::Ready)
                }
                Err(err) => {
                    if is_auth_challenge(&err) {
                        // The server answered with an auth request: it is up.
                        Ok(Health::Ready)
                    } else {
                        Err(DblError::Engine {
                            reason: format!("postgres connection failed: {err}"),
                        })
                    }
                }
            }
        })
    }

    fn shutdown(&self, ctx: &RunContext) -> Result<()> {
        let pg_ctl = ctx.runtime_dir.join("bin").join(exe_name("pg_ctl"));
        let status = Command::new(pg_ctl)
            .arg("-D")
            .arg(&ctx.data_dir)
            .arg("-m")
            .arg("fast")
            .arg("-w")
            .arg("-t")
            .arg(consts::STOP_GRACE_SECS.to_string())
            .arg("stop")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .map_err(|e| DblError::Engine {
                reason: format!("failed to execute pg_ctl stop: {e}"),
            })?;
        if status.status.success() {
            return Ok(());
        }
        let tail = combined_output(status.stdout, status.stderr);
        Err(DblError::Engine {
            reason: format!(
                "pg_ctl stop failed (code {:?}): {}",
                status.status.code(),
                tail.trim_end()
            ),
        })
    }

    fn connection_uri(
        &self,
        ctx: &RunContext,
        credential: &Credential,
        format: UriFormat,
    ) -> SecretString {
        uri::render(EngineKey::Postgresql, ctx.port.get(), credential, format)
    }

    fn diagnose_start_failure(&self, output: &str, _exit_code: Option<i32>) -> StartFailure {
        diagnose_failure_text(output)
    }

    fn admin_ops(&self) -> Option<&dyn AdminOps> {
        // Phase 2.
        None
    }
}

/// Identity parsed from a PostgreSQL `postmaster.pid` (spike section 10).
///
/// Line 1 is the PID, line 3 is the postmaster start epoch in seconds. The
/// executor must also verify the executable resolves to the expected
/// `postgres` binary and that `/proc/<pid>` start time matches this epoch
/// before signalling (C-08).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PostmasterIdentity {
    /// Postmaster process id (line 1).
    pub pid: u32,
    /// Data directory recorded in the pidfile (line 2).
    pub data_dir: PathBuf,
    /// Postmaster start timestamp, epoch seconds (line 3).
    pub start_epoch: i64,
    /// Listening port (line 4).
    pub port: u16,
    /// Unix socket directories (line 5).
    pub socket_dirs: String,
    /// `ready` or `stopping` status token (line 8), when present.
    pub status: Option<String>,
}

/// Parses the contents of `postmaster.pid`.
pub fn parse_postmaster_pid(contents: &str) -> Result<PostmasterIdentity> {
    let lines: Vec<&str> = contents.lines().collect();
    if lines.len() < 6 {
        return Err(DblError::Engine {
            reason: "postmaster.pid is incomplete".to_owned(),
        });
    }
    let pid = lines[0]
        .trim()
        .parse::<u32>()
        .map_err(|_| DblError::Engine {
            reason: "postmaster.pid has an invalid pid".to_owned(),
        })?;
    let start_epoch = lines[2]
        .trim()
        .parse::<i64>()
        .map_err(|_| DblError::Engine {
            reason: "postmaster.pid has an invalid start time".to_owned(),
        })?;
    let port = lines[3]
        .trim()
        .parse::<u16>()
        .map_err(|_| DblError::Engine {
            reason: "postmaster.pid has an invalid port".to_owned(),
        })?;
    let status = lines
        .get(7)
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty());
    Ok(PostmasterIdentity {
        pid,
        data_dir: PathBuf::from(lines[1].trim()),
        start_epoch,
        port,
        socket_dirs: lines[4].trim().to_owned(),
        status,
    })
}

/// Reads and parses `<data_dir>/postmaster.pid`.
pub fn read_postmaster_pid(data_dir: &Path) -> Result<PostmasterIdentity> {
    let path = data_dir.join("postmaster.pid");
    let contents = fs::read_to_string(&path).map_err(|e| DblError::Engine {
        reason: format!("cannot read postmaster.pid: {e}"),
    })?;
    parse_postmaster_pid(&contents)
}

/// Returns the log file path for an instance.
pub fn log_path(ctx: &RunContext) -> PathBuf {
    ctx.logs_dir.join("postgres.log")
}

/// Returns the unix socket directory for an instance.
///
/// Kept under `run_dir/sock` so the full socket path
/// `<dir>/.s.PGSQL.<port>` can stay short and be checked against the
/// 107-byte AF_UNIX limit (C-09).
pub fn socket_dir_for_run(run_dir: &Path) -> PathBuf {
    run_dir.join("sock")
}

/// Computes and validates the unix socket directory for init.
fn socket_dir_for_init(ctx: &InitContext) -> Result<PathBuf> {
    let run_dir = ctx
        .data_dir
        .parent()
        .map(|p| p.join(consts::DIR_RUN))
        .unwrap_or_else(|| ctx.data_dir.join(consts::DIR_RUN));
    let dir = socket_dir_for_run(&run_dir);
    validate_socket_path_len(&dir, ctx.port.get())?;
    Ok(dir)
}

/// Ensures the unix socket directory exists with restrictive permissions.
pub fn ensure_socket_dir(dir: &Path) -> Result<()> {
    fs::create_dir_all(dir).map_err(|e| DblError::Engine {
        reason: format!("cannot create unix socket directory {}: {e}", dir.display()),
    })?;
    #[cfg(unix)]
    {
        let mut perms = fs::metadata(dir)
            .map_err(|e| DblError::Engine {
                reason: format!("cannot stat unix socket directory: {e}"),
            })?
            .permissions();
        perms.set_mode(0o700);
        fs::set_permissions(dir, perms).map_err(|e| DblError::Engine {
            reason: format!("cannot set unix socket directory permissions: {e}"),
        })?;
    }
    Ok(())
}

/// Validates that `<dir>/.s.PGSQL.<port>` fits within the AF_UNIX limit.
pub fn validate_socket_path_len(dir: &Path, port: u16) -> Result<()> {
    let sock = dir.join(format!(".s.PGSQL.{port}"));
    let len = sock.as_os_str().as_encoded_bytes().len();
    if len > UNIX_SOCKET_PATH_LIMIT {
        return Err(DblError::InvalidInput {
            field: "unix_socket_directories",
            reason: format!(
                "unix socket path {} is {} bytes, maximum is {}",
                sock.display(),
                len,
                UNIX_SOCKET_PATH_LIMIT
            ),
        });
    }
    Ok(())
}

#[derive(Clone, Copy)]
struct AuthArgs {
    local: &'static str,
    host: &'static str,
}

fn auth_args(mode: AuthMode) -> AuthArgs {
    match mode {
        AuthMode::Password => AuthArgs {
            local: "scram-sha-256",
            host: "scram-sha-256",
        },
        AuthMode::None => AuthArgs {
            local: "trust",
            host: "trust",
        },
    }
}

fn effective_superuser(ctx: &InitContext) -> String {
    if ctx.superuser.is_empty() {
        consts::SUPERUSER_POSTGRESQL.to_owned()
    } else {
        ctx.superuser.clone()
    }
}

fn write_pwfile(data_dir: &Path, password: Option<&SecretString>) -> Result<PathBuf> {
    let pw = password.ok_or_else(|| DblError::InvalidInput {
        field: "password",
        reason: "password auth requires a generated password".to_owned(),
    })?;
    let base = data_dir.parent().unwrap_or(data_dir);
    fs::create_dir_all(base).map_err(|e| DblError::Engine {
        reason: format!("cannot create password file directory: {e}"),
    })?;
    let path = base.join(".dblocal-pwfile");
    let mut opts = OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut file = opts.open(&path).map_err(|e| DblError::Engine {
        reason: format!("cannot create initdb password file: {e}"),
    })?;
    file.write_all(pw.expose().as_bytes())
        .map_err(|e| DblError::Engine {
            reason: format!("cannot write initdb password file: {e}"),
        })?;
    file.write_all(b"\n").map_err(|e| DblError::Engine {
        reason: format!("cannot write initdb password file: {e}"),
    })?;
    file.sync_all().map_err(|e| DblError::Engine {
        reason: format!("cannot sync initdb password file: {e}"),
    })?;
    #[cfg(unix)]
    {
        let perms = fs::Permissions::from_mode(0o600);
        fs::set_permissions(&path, perms).map_err(|e| DblError::Engine {
            reason: format!("cannot chmod initdb password file: {e}"),
        })?;
    }
    Ok(path)
}

fn configure_after_init(ctx: &InitContext, socket_dir: &Path) -> Result<()> {
    let conf = ctx.data_dir.join("postgresql.conf");
    let mut file = OpenOptions::new()
        .append(true)
        .open(&conf)
        .map_err(|e| DblError::Engine {
            reason: format!("cannot open postgresql.conf: {e}"),
        })?;
    writeln!(file).ok();
    writeln!(file, "# DBLocal managed settings").ok();
    writeln!(file, "listen_addresses = '127.0.0.1'").ok();
    writeln!(file, "port = {}", ctx.port.get()).ok();
    writeln!(
        file,
        "unix_socket_directories = '{}'",
        escape_single_quotes(&socket_dir.display().to_string())
    )
    .ok();
    file.sync_all().map_err(|e| DblError::Engine {
        reason: format!("cannot sync postgresql.conf: {e}"),
    })?;
    Ok(())
}

fn escape_single_quotes(value: &str) -> String {
    value.replace('\'', "''")
}

fn initdb_failed(stdout: Vec<u8>, stderr: Vec<u8>, code: Option<i32>) -> DblError {
    let tail = combined_output(stdout, stderr);
    DblError::Engine {
        reason: format!("initdb failed (code {:?}): {}", code, tail.trim_end()),
    }
}

fn combined_output(stdout: Vec<u8>, stderr: Vec<u8>) -> String {
    let mut out = String::from_utf8_lossy(&stdout).into_owned();
    let err = String::from_utf8_lossy(&stderr);
    if !err.is_empty() {
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(&err);
    }
    out
}

/// Chooses the first UTF-8 locale available on the host, falling back to `C`.
///
/// Preference order: `C.UTF-8`, `C.utf8`, `C` (spike section 3).
pub fn select_locale() -> Result<String> {
    let output = Command::new("locale")
        .arg("-a")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output();
    let available = match output {
        Ok(out) if out.status.success() => parse_locale_list(&String::from_utf8_lossy(&out.stdout)),
        _ => HashSet::new(),
    };
    Ok(pick_locale(&available).to_owned())
}

fn parse_locale_list(text: &str) -> HashSet<String> {
    text.lines()
        .map(|l| l.trim().to_owned())
        .filter(|l| !l.is_empty())
        .collect()
}

fn pick_locale(available: &HashSet<String>) -> &'static str {
    if available.contains("C.UTF-8") {
        "C.UTF-8"
    } else if available.contains("C.utf8") {
        "C.utf8"
    } else {
        "C"
    }
}

/// Classifies raw PostgreSQL start/init output into a [`StartFailure`].
///
/// Uses plain substring matching on the stable markers from the spike's
/// verbatim failure texts.
pub fn diagnose_failure_text(output: &str) -> StartFailure {
    // Port in use: TCP bind failures and the specific HINT/FATAL forms.
    if output.contains("could not bind IPv4 address")
        || output.contains("Is another postmaster already running on port")
        || (output.contains("Address already in use")
            && output.contains("could not create any TCP/IP sockets"))
        || output.contains("could not create any TCP/IP sockets")
            && output.contains("Address already in use")
    {
        return StartFailure::PortInUse;
    }
    // Strong TCP socket creation failure without another marker still means
    // the server could not bind a TCP socket (spike 11.1).
    if output.contains("could not create any TCP/IP sockets") {
        return StartFailure::PortInUse;
    }

    if output.contains("database files are incompatible with server")
        || output.contains("initialized by PostgreSQL version")
    {
        return StartFailure::DataDirVersionMismatch;
    }

    // Explicit permission denials.
    if output.contains("Permission denied") {
        return StartFailure::PermissionDenied;
    }

    // Socket directory problems: too-long path or missing socket dir/lock file.
    // These are path/configuration issues surfaced in logs; classify as
    // Unknown only when not a clear permission case, but they are recognized.
    if output.contains("Unix-domain socket path") && output.contains("is too long") {
        // Recognized socket-dir problem; no dedicated variant exists.
        return StartFailure::PermissionDenied;
    }
    if output.contains(".s.PGSQL.") && output.contains("No such file or directory") {
        // Missing socket directory / lock file path.
        return StartFailure::PermissionDenied;
    }

    // Missing runtime/data directory or missing/corrupt binary.
    if is_binary_missing(output) {
        return StartFailure::BinaryMissing;
    }

    StartFailure::Unknown
}

fn is_binary_missing(output: &str) -> bool {
    // pg_ctl pointing at a nonexistent data directory.
    if output.contains("pg_ctl: directory \"") && output.contains("does not exist") {
        return true;
    }
    // Shell/OS exec failures.
    if output.contains("No such file or directory") {
        return true;
    }
    if output.contains("Exec format error") {
        return true;
    }
    false
}

fn strip_password_env(cmd: &mut Command) {
    // Never forward credentials in the environment to engine processes.
    cmd.env_remove("PGPASSWORD");
    cmd.env_remove("PGPASSFILE");
}

fn is_auth_challenge(err: &tokio_postgres::Error) -> bool {
    if let Some(db) = err.as_db_error() {
        let code = db.code().code();
        // 28P01 invalid_password, 28000 invalid_authorization_specification,
        // 28Pxx and 28000 family indicate the server spoke the protocol and
        // challenged authentication - proof it is up.
        if code == "28P01" || code == "28000" || code.starts_with("28") {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::InitContext;
    use crate::model::Port;
    use std::ffi::OsString;
    use tempfile::tempdir;

    fn init_ctx(auth: AuthMode, password: Option<&str>) -> InitContext {
        InitContext {
            instance_name: "pg-test".into(),
            runtime_dir: PathBuf::from("/tmp/rt"),
            data_dir: PathBuf::from("/tmp/data"),
            port: Port::new(41010).expect("port"),
            superuser: "postgres".into(),
            auth,
            password: password.map(SecretString::from),
        }
    }

    #[test]
    fn auth_args_match_mode() {
        let pw = auth_args(AuthMode::Password);
        assert_eq!(pw.local, "scram-sha-256");
        assert_eq!(pw.host, "scram-sha-256");
        let none = auth_args(AuthMode::None);
        assert_eq!(none.local, "trust");
        assert_eq!(none.host, "trust");
    }

    #[test]
    fn socket_path_limit_enforced() {
        let short = Path::new("/tmp/s");
        assert!(validate_socket_path_len(short, 41010).is_ok());
        let long_dir = "/".to_owned() + &"a".repeat(120);
        let err = validate_socket_path_len(Path::new(&long_dir), 41010).unwrap_err();
        assert!(matches!(err, DblError::InvalidInput { .. }));
    }

    #[test]
    fn socket_path_boundary() {
        // Build a dir whose resulting socket path is exactly at the limit.
        let port = 41010u16;
        let suffix = format!("/.s.PGSQL.{port}");
        let dir_len = UNIX_SOCKET_PATH_LIMIT - suffix.len();
        let dir = PathBuf::from("/".to_owned() + &"s".repeat(dir_len.saturating_sub(1)));
        assert!(validate_socket_path_len(&dir, port).is_ok());
    }

    #[test]
    fn parse_postmaster_pid_happy_path() {
        let contents = "3810050\n/tmp/data\n1795176359\n41060\n/tmp/sock\n127.0.0.1\n  2558019     32796\nready\n";
        let id = parse_postmaster_pid(contents).expect("parse");
        assert_eq!(id.pid, 3810050);
        assert_eq!(id.data_dir, PathBuf::from("/tmp/data"));
        assert_eq!(id.start_epoch, 1795176359);
        assert_eq!(id.port, 41060);
        assert_eq!(id.socket_dirs, "/tmp/sock");
        assert_eq!(id.status.as_deref(), Some("ready"));
    }

    #[test]
    fn parse_postmaster_pid_rejects_short_file() {
        assert!(parse_postmaster_pid("1\n2\n3\n").is_err());
        assert!(parse_postmaster_pid("abc\n/tmp\n1\n1\n/tmp\n127.0.0.1\n").is_err());
    }

    #[test]
    fn read_postmaster_pid_from_file() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("postmaster.pid");
        fs::write(&path, "42\n/d\n100\n5432\n/s\n127.0.0.1\n0 0\nready\n").expect("write");
        let id = read_postmaster_pid(dir.path()).expect("read");
        assert_eq!(id.pid, 42);
    }

    #[test]
    fn locale_preference_order() {
        let mut set = HashSet::new();
        set.insert("C".to_owned());
        set.insert("en_US.UTF-8".to_owned());
        assert_eq!(pick_locale(&set), "C");
        set.insert("C.utf8".to_owned());
        assert_eq!(pick_locale(&set), "C.utf8");
        set.insert("C.UTF-8".to_owned());
        assert_eq!(pick_locale(&set), "C.UTF-8");
    }

    #[test]
    fn parse_locale_list_trims() {
        let set = parse_locale_list("C\nC.UTF-8\nen_US.UTF-8\n");
        assert!(set.contains("C.UTF-8"));
        assert_eq!(set.len(), 3);
    }

    #[test]
    fn combined_output_merges_streams() {
        let out = combined_output(b"out\n".to_vec(), b"err\n".to_vec());
        assert!(out.contains("out"));
        assert!(out.contains("err"));
    }

    #[test]
    fn write_pwfile_creates_restricted_file() {
        let dir = tempdir().expect("tempdir");
        let data_dir = dir.path().join("data");
        fs::create_dir_all(&data_dir).expect("data");
        let pw = SecretString::from("s3cret");
        let path = write_pwfile(&data_dir, Some(&pw)).expect("pwfile");
        let contents = fs::read_to_string(&path).expect("read");
        assert_eq!(contents, "s3cret\n");
        #[cfg(unix)]
        {
            let mode = fs::metadata(&path).expect("meta").permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        let _ = fs::remove_file(path);
    }

    #[test]
    fn init_ctx_requires_password_when_auth_is_password() {
        let ctx = init_ctx(AuthMode::Password, None);
        assert!(ctx.with_password());
        assert!(ctx.password.is_none());
    }

    #[test]
    fn spawn_command_does_not_carry_password() {
        let ctx = RunContext {
            instance_name: "app".into(),
            runtime_dir: PathBuf::from("/opt/pg"),
            data_dir: PathBuf::from("/data"),
            logs_dir: PathBuf::from("/logs"),
            run_dir: PathBuf::from("/run"),
            port: Port::new(41010).expect("port"),
            superuser: "postgres".into(),
            auth: AuthMode::Password,
            credential: Some(Credential::new(
                "postgres",
                SecretString::from("top-secret"),
            )),
        };
        let cmd = PostgresAdapter.spawn_command(&ctx);
        let args: Vec<OsString> = cmd.get_args().map(|s| s.to_os_string()).collect();
        for a in &args {
            assert!(!a.to_string_lossy().contains("top-secret"));
        }
        for (k, v) in cmd.get_envs() {
            if let Some(v) = v {
                assert!(!v.to_string_lossy().contains("top-secret"));
                assert_ne!(k.to_string_lossy(), "PGPASSWORD");
            }
        }
        let prog = cmd.get_program().to_string_lossy();
        assert!(prog.ends_with("pg_ctl") || prog.ends_with("pg_ctl.exe"));
    }

    #[test]
    fn configure_after_init_writes_managed_settings() {
        let dir = tempdir().expect("tempdir");
        let data = dir.path().join("data");
        fs::create_dir_all(&data).expect("data");
        fs::write(data.join("postgresql.conf"), "# base\n").expect("conf");
        let ctx = InitContext {
            instance_name: "x".into(),
            runtime_dir: PathBuf::from("/rt"),
            data_dir: data.clone(),
            port: Port::new(41010).expect("port"),
            superuser: "postgres".into(),
            auth: AuthMode::None,
            password: None,
        };
        let sock = dir.path().join("run/sock");
        configure_after_init(&ctx, &sock).expect("configure");
        let text = fs::read_to_string(data.join("postgresql.conf")).expect("read");
        assert!(text.contains("listen_addresses = '127.0.0.1'"));
        assert!(text.contains("port = 41010"));
        assert!(text.contains("unix_socket_directories = '"));
    }

    #[test]
    fn log_path_is_under_logs_dir() {
        let ctx = RunContext {
            instance_name: "app".into(),
            runtime_dir: PathBuf::from("/rt"),
            data_dir: PathBuf::from("/d"),
            logs_dir: PathBuf::from("/logs"),
            run_dir: PathBuf::from("/run"),
            port: Port::new(41010).expect("port"),
            superuser: "postgres".into(),
            auth: AuthMode::None,
            credential: None,
        };
        assert_eq!(log_path(&ctx), PathBuf::from("/logs/postgres.log"));
    }

    #[test]
    fn escape_single_quotes_doubles() {
        assert_eq!(escape_single_quotes("a'b"), "a''b");
    }

    // ---- Table-driven diagnosis: every verbatim failure text from spike ----

    fn cases() -> Vec<(&'static str, StartFailure)> {
        vec![
            // Port in use (spike 11.1)
            (
                "LOG:  could not bind IPv4 address \"127.0.0.1\": Address already in use\nHINT:  Is another postmaster already running on port 41100? If not, wait a few seconds and retry.\nWARNING:  could not create listen socket for \"127.0.0.1\"\nFATAL:  could not create any TCP/IP sockets\nLOG:  database system is shut down\n",
                StartFailure::PortInUse,
            ),
            (
                "HINT:  Is another postmaster already running on port 41100? If not, wait a few seconds and retry.",
                StartFailure::PortInUse,
            ),
            // Wrong-major data dir (spike 11.2)
            (
                "FATAL:  database files are incompatible with server\nDETAIL:  The data directory was initialized by PostgreSQL version 17, which is not compatible with this version 18.7.\n",
                StartFailure::DataDirVersionMismatch,
            ),
            // Permission denied (spike 11.3)
            (
                "pg_ctl: could not open PID file \"/tmp/d/postmaster.pid\": Permission denied",
                StartFailure::PermissionDenied,
            ),
            (
                "FATAL:  could not create lock file \"postmaster.pid\": Permission denied",
                StartFailure::PermissionDenied,
            ),
            // Socket dir missing (spike 11.5) - path problem, recognized
            (
                "FATAL:  could not create lock file \"/tmp/s5/.s.PGSQL.41130.lock\": No such file or directory",
                StartFailure::PermissionDenied,
            ),
            // Socket path too long (spike 11.5)
            (
                "LOG:  Unix-domain socket path \"/tmp/very/long/.s.PGSQL.41000\" is too long (maximum 107 bytes)\nFATAL:  could not create any Unix-domain sockets\n",
                StartFailure::PermissionDenied,
            ),
            // Missing/corrupt binary contexts (spike 11.4)
            (
                "pg_ctl: directory \"/tmp/nodir\" does not exist",
                StartFailure::BinaryMissing,
            ),
            (
                "/bin/sh: line 1: /tmp/missing: No such file or directory",
                StartFailure::BinaryMissing,
            ),
            (
                "bash: ./postgres: Exec format error",
                StartFailure::BinaryMissing,
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
            diagnose_failure_text("something completely different\n"),
            StartFailure::Unknown
        );
    }
}
