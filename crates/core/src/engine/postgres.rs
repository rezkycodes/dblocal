//! PostgreSQL engine adapter.
//!
//! Implements initdb bootstrap, config rendering, `pg_ctl` start/stop command
//! construction, `postmaster.pid` identity parsing, protocol health checks and
//! start-failure diagnosis from the spike findings (C-06, C-07, C-08, C-09).

use std::collections::HashSet;
use std::fmt::Write as _;
use std::fs::{self, OpenOptions};
use std::io::Write as IoWrite;
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

/// How PostgreSQL will expose a unix-domain socket for one instance.
///
/// DBLocal always connects over TCP loopback, so the unix socket is optional. On
/// a deep home path the full `<dir>/.s.PGSQL.<port>` path can exceed the kernel's
/// 107-byte `sun_path` limit; rather than failing instance creation we disable
/// it and stay TCP-only (C-09).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SocketDir {
    /// Unix sockets enabled at this directory.
    Enabled(PathBuf),
    /// Unix sockets disabled; `unix_socket_directories` is rendered empty.
    Disabled,
}

impl SocketDir {
    /// Path used for the socket, or `None` when unix sockets are disabled.
    pub fn path(&self) -> Option<&Path> {
        match self {
            Self::Enabled(dir) => Some(dir),
            Self::Disabled => None,
        }
    }

    /// Value written to `unix_socket_directories`; empty disables the socket.
    pub fn config_value(&self) -> String {
        match self {
            Self::Enabled(dir) => dir.display().to_string(),
            Self::Disabled => String::new(),
        }
    }
}

/// Chooses the unix socket directory, falling back to TCP-only when too long.
///
/// This is the single place that applies the 107-byte check: an over-long path
/// disables the unix socket instead of failing creation, and the choice is
/// logged so a support log shows which path was taken.
pub fn choose_socket_dir(dir: &Path, port: u16) -> SocketDir {
    choose_socket_dir_for(dir, port, cfg!(windows))
}

/// [`choose_socket_dir`] with the platform passed in, so both are testable.
///
/// PostgreSQL has no unix sockets on Windows. Writing `unix_socket_directories`
/// there made the server try to create a lock file in a path whose backslashes
/// the configuration parser had consumed as escapes (`FATAL: could not create
/// lock file "C:dblhomeinstances..."`), so Windows stays TCP-only.
fn choose_socket_dir_for(dir: &Path, port: u16, windows: bool) -> SocketDir {
    if windows {
        return SocketDir::Disabled;
    }
    match validate_socket_path_len(dir, port) {
        Ok(()) => SocketDir::Enabled(dir.to_path_buf()),
        Err(err) => {
            tracing::info!(
                socket_dir = %dir.display(),
                "unix socket path exceeds 107 bytes; disabling unix sockets and using TCP loopback only (C-09): {err}"
            );
            SocketDir::Disabled
        }
    }
}

/// PostgreSQL adapter.
#[derive(Debug, Clone, Copy, Default)]
pub struct PostgresAdapter;

/// Prepares the socket directory for a start and returns the chosen plan.
///
/// `spawn_command` cannot fail, so the service calls this before spawning: it
/// re-checks `<run_dir>/sock/.s.PGSQL.<port>` against the 107-byte limit at
/// start time as well as create time (C-09) and makes sure the directory the
/// server writes its socket lock into exists and is writable. An over-long path
/// yields [`SocketDir::Disabled`] (TCP-only) instead of an error.
pub fn prepare_run(ctx: &RunContext) -> Result<SocketDir> {
    let plan = choose_socket_dir(&socket_dir_for_run(&ctx.run_dir), ctx.port.get());
    if let Some(dir) = plan.path() {
        ensure_socket_dir(dir)?;
    }
    Ok(plan)
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
        let socket_plan = socket_dir_plan_for_init(ctx);
        if let Some(dir) = socket_plan.path() {
            ensure_socket_dir(dir)?;
        }
        let locale = select_locale()?;
        tracing::debug!(
            instance = %ctx.instance_name,
            locale = %locale,
            socket_dir = ?socket_plan.path(),
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

        let outcome = crate::fsutil::output_retrying_text_file_busy(cmd);
        let init_result = match outcome {
            Ok(out) if out.status.success() => configure_after_init(ctx, &socket_plan),
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
        cmd.args(pg_ctl_start_args(ctx, cfg!(windows)));
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
        let mut cmd = Command::new(pg_ctl);
        cmd.arg("-D")
            .arg(&ctx.data_dir)
            .arg("-m")
            .arg("fast")
            .arg("-w")
            .arg("-t")
            .arg(consts::STOP_GRACE_SECS.to_string())
            .arg("stop")
            .stdin(Stdio::null());
        let status =
            crate::fsutil::output_retrying_text_file_busy(cmd).map_err(|e| DblError::Engine {
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

/// Arguments of `pg_ctl start`.
///
/// The supervisor already appends the child's stdout and stderr to the log file
/// (`process::spawn_detached`), and the server inherits them. On Unix `-l` names
/// that same file, which is harmless. On Windows `pg_ctl -l` makes `cmd.exe`
/// open the file again with `>>`, and that open is refused with "The process
/// cannot access the file because it is being used by another process" while our
/// handle is open, so the start failed with `DBL-4001`. Windows therefore relies
/// on the inherited handles alone.
fn pg_ctl_start_args(ctx: &RunContext, windows: bool) -> Vec<std::ffi::OsString> {
    let mut args: Vec<std::ffi::OsString> = vec!["-D".into(), ctx.data_dir.clone().into()];
    if !windows {
        args.push("-l".into());
        args.push(log_path(ctx).into());
    }
    args.extend([
        "-w".into(),
        "-t".into(),
        consts::START_TIMEOUT_SECS.to_string().into(),
        "start".into(),
    ]);
    args
}

/// Returns the unix socket directory for an instance.
///
/// Kept under `run_dir/sock` so the full socket path
/// `<dir>/.s.PGSQL.<port>` can stay short and be checked against the
/// 107-byte AF_UNIX limit (C-09).
pub fn socket_dir_for_run(run_dir: &Path) -> PathBuf {
    run_dir.join("sock")
}

/// Computes the unix socket plan for init, disabling it when too long (C-09).
fn socket_dir_plan_for_init(ctx: &InitContext) -> SocketDir {
    let run_dir = ctx
        .data_dir
        .parent()
        .map(|p| p.join(consts::DIR_RUN))
        .unwrap_or_else(|| ctx.data_dir.join(consts::DIR_RUN));
    choose_socket_dir(&socket_dir_for_run(&run_dir), ctx.port.get())
}

/// Ensures the unix socket directory exists with restrictive permissions.
pub fn ensure_socket_dir(dir: &Path) -> Result<()> {
    crate::fsutil::ensure_private_dir(dir).map_err(|e| DblError::Engine {
        reason: format!("cannot create unix socket directory {}: {e}", dir.display()),
    })
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

const MANAGED_BEGIN: &str = "# DBLocal managed settings";
const MANAGED_END: &str = "# DBLocal managed settings end";

fn managed_block(port: u16, socket_plan: &SocketDir) -> String {
    let mut block = String::new();
    let _ = writeln!(block, "{MANAGED_BEGIN}");
    let _ = writeln!(block, "listen_addresses = '127.0.0.1'");
    let _ = writeln!(block, "port = {port}");
    let _ = writeln!(
        block,
        "unix_socket_directories = '{}'",
        escape_single_quotes(&socket_plan.config_value())
    );
    let _ = write!(block, "{MANAGED_END}");
    block
}

/// Appends the DBLocal-managed settings block after initdb has written its base config.
///
/// Public for integration-style tests that need a real initdb-shaped config file.
pub fn configure_after_init(ctx: &InitContext, socket_plan: &SocketDir) -> Result<()> {
    let conf = ctx.data_dir.join("postgresql.conf");
    let mut contents = fs::read_to_string(&conf).map_err(|e| DblError::Engine {
        reason: format!("cannot read postgresql.conf: {e}"),
    })?;
    if !contents.ends_with('\n') {
        contents.push('\n');
    }
    contents.push('\n');
    contents.push_str(&managed_block(ctx.port.get(), socket_plan));
    contents.push('\n');
    write_private_config_atomic(&conf, &contents)
}

/// Rewrites the DBLocal-managed port/socket settings in `postgresql.conf`.
///
/// Atomic: writes a temp file in the same directory, fsyncs it, then renames.
/// Other settings and comments are preserved. If the managed block is missing
/// (older layout), it is appended. Idempotent.
pub fn sync_port(data_dir: &Path, run_dir: &Path, port: u16) -> Result<SocketDir> {
    let plan = choose_socket_dir(&socket_dir_for_run(run_dir), port);
    if let Some(dir) = plan.path() {
        ensure_socket_dir(dir)?;
    }
    let conf = data_dir.join("postgresql.conf");
    let contents = fs::read_to_string(&conf).map_err(|e| DblError::Engine {
        reason: format!("cannot read postgresql.conf: {e}"),
    })?;
    let block = managed_block(port, &plan);
    let updated = replace_managed_block(&contents, &block, port);
    write_private_config_atomic(&conf, &updated)?;
    Ok(plan)
}

fn replace_managed_block(contents: &str, block: &str, port: u16) -> String {
    if let Some(start) = contents.find(MANAGED_BEGIN)
        && let Some(end_rel) = contents[start..].find(MANAGED_END)
    {
        let end = start + end_rel + MANAGED_END.len();
        let mut out = String::with_capacity(contents.len() + block.len());
        out.push_str(&contents[..start]);
        out.push_str(block);
        out.push_str(&contents[end..]);
        return out;
    }
    // Legacy layout: replace individual managed keys if present, else append.
    let mut replaced_port = false;
    let mut replaced_socket = false;
    let mut replaced_listen = false;
    let mut lines: Vec<String> = contents.lines().map(|l| l.to_owned()).collect();
    for line in &mut lines {
        let trimmed = line.trim_start();
        if trimmed.starts_with("port =") {
            *line = format!("port = {port}");
            replaced_port = true;
        } else if trimmed.starts_with("unix_socket_directories =") {
            *line = extract_line(block, "unix_socket_directories =").to_owned();
            replaced_socket = true;
        } else if trimmed.starts_with("listen_addresses =") && trimmed.contains("127.0.0.1") {
            *line = "listen_addresses = '127.0.0.1'".to_owned();
            replaced_listen = true;
        }
    }
    if replaced_port && replaced_socket && replaced_listen {
        let mut out = lines.join("\n");
        if contents.ends_with('\n') {
            out.push('\n');
        }
        return out;
    }
    let mut out = contents.to_owned();
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out.push('\n');
    out.push_str(block);
    out.push('\n');
    out
}

fn extract_line<'a>(block: &'a str, prefix: &str) -> &'a str {
    block
        .lines()
        .find(|l| l.trim_start().starts_with(prefix))
        .unwrap_or("")
}

fn write_private_config_atomic(path: &Path, contents: &str) -> Result<()> {
    crate::fsutil::write_private_file_atomic(path, contents.as_bytes()).map_err(|e| {
        DblError::Engine {
            reason: format!("cannot write {}: {e}", path.display()),
        }
    })
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
    // These are recognized but have no honest StartFailure variant, so we
    // return Unknown and keep the raw log tail for F-LOG-02.
    if output.contains("Unix-domain socket path") && output.contains("is too long") {
        return StartFailure::Unknown;
    }
    if output.contains(".s.PGSQL.") && output.contains("No such file or directory") {
        return StartFailure::Unknown;
    }

    // Missing runtime/data directory or missing/corrupt binary.
    if is_binary_missing(output) {
        return StartFailure::BinaryMissing;
    }

    StartFailure::Unknown
}

fn is_binary_missing(output: &str) -> bool {
    // Only genuine exec/binary failures count. A nonexistent data directory
    // ("pg_ctl: directory \"...\" does not exist") is not a missing binary, so
    // it stays Unknown and keeps the raw tail for F-LOG-02.
    if output.contains("Exec format error") {
        return true;
    }
    // Shell/OS exec failure, but not a socket-dir lock-file error (handled
    // above as Unknown).
    output.contains("No such file or directory") && !output.contains(".s.PGSQL.")
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
mod tests;
