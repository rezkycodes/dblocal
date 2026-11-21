//! Pure launch planning (F-OPN-02).

use std::path::{Path, PathBuf};

use super::error::{ClientError, ClientResult};
use crate::model::EngineKey;
use crate::secret::SecretString;

use super::detect::{ClientKind, DetectedClient};
use super::quote::shell_quote;
use super::terminal::{SCRIPT_PLACEHOLDER, escape_applescript, launcher_temp_file};

/// Target instance connection information.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenTarget {
    engine: EngineKey,
    host: String,
    port: u16,
    username: String,
    database: String,
    instance_name: String,
}

impl OpenTarget {
    /// Creates an open target. Values are treated as untrusted input and validated.
    pub fn new(
        engine: EngineKey,
        host: impl Into<String>,
        port: u16,
        username: impl Into<String>,
        database: impl Into<String>,
        instance_name: impl Into<String>,
    ) -> ClientResult<Self> {
        let host = host.into();
        let username = username.into();
        let database = database.into();
        let instance_name = instance_name.into();
        validate_host(&host)?;
        validate_identifier(&username, "username", false)?;
        validate_identifier(&database, "database", true)?;
        validate_instance_name(&instance_name)?;
        if !(1..=u16::MAX).contains(&port) {
            return Err(ClientError::InvalidTarget { field: "port" });
        }
        Ok(Self {
            engine,
            host,
            port,
            username,
            database,
            instance_name,
        })
    }

    pub fn engine(&self) -> EngineKey {
        self.engine
    }
    pub fn host(&self) -> &str {
        &self.host
    }
    pub fn port(&self) -> u16 {
        self.port
    }
    pub fn username(&self) -> &str {
        &self.username
    }
    pub fn database(&self) -> &str {
        &self.database
    }
    pub fn instance_name(&self) -> &str {
        &self.instance_name
    }
}

fn validate_host(host: &str) -> ClientResult<()> {
    let ok = matches!(host, "127.0.0.1" | "::1" | "localhost");
    if ok {
        Ok(())
    } else {
        Err(ClientError::InvalidTarget { field: "host" })
    }
}

fn validate_identifier(value: &str, field: &'static str, allow_empty: bool) -> ClientResult<()> {
    if value.is_empty() {
        return if allow_empty {
            Ok(())
        } else {
            Err(ClientError::InvalidTarget { field })
        };
    }
    if value.len() > 63 || value.starts_with('-') {
        return Err(ClientError::InvalidTarget { field });
    }
    let ok = value
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'));
    if ok {
        Ok(())
    } else {
        Err(ClientError::InvalidTarget { field })
    }
}

fn validate_instance_name(value: &str) -> ClientResult<()> {
    if value.is_empty() || value.len() > 128 || value.chars().any(|c| c.is_control()) {
        return Err(ClientError::InvalidTarget {
            field: "instance_name",
        });
    }
    Ok(())
}
/// OS family for deterministic plan generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OsKind {
    Linux,
    Macos,
    Windows,
}

impl OsKind {
    pub const fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::Macos
        } else if cfg!(target_os = "windows") {
            Self::Windows
        } else {
            Self::Linux
        }
    }
}

/// How a CLI client is launched in a terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalChoice {
    program: PathBuf,
    kind: TerminalKind,
}

impl TerminalChoice {
    pub fn new(program: impl Into<PathBuf>, kind: TerminalKind) -> Self {
        Self {
            program: program.into(),
            kind,
        }
    }
    pub fn program(&self) -> &PathBuf {
        &self.program
    }
    pub fn kind(&self) -> TerminalKind {
        self.kind
    }
}

/// Supported terminal emulator kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TerminalKind {
    GenericArgv,
    GnomeTerminal,
    Konsole,
    Kitty,
    Alacritty,
    Xterm,
    Foot,
    Wezterm,
    XdgTerminalExec,
    Ptyxis,
    MacOsTerminalApp,
    WindowsTerminal,
    WindowsCmd,
}

/// Environment variable to set on the spawned process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchEnv {
    name: String,
    value: SecretString,
}

impl LaunchEnv {
    pub fn new(name: impl Into<String>, value: SecretString) -> Self {
        Self {
            name: name.into(),
            value,
        }
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn value(&self) -> &SecretString {
        &self.value
    }
}

/// A private temporary file the spawner should create before launch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TempFile {
    name_hint: String,
    contents: SecretString,
    mode: u32,
}

impl TempFile {
    pub fn new(name_hint: impl Into<String>, contents: SecretString, mode: u32) -> Self {
        Self {
            name_hint: name_hint.into(),
            contents,
            mode,
        }
    }
    pub fn name_hint(&self) -> &str {
        &self.name_hint
    }
    pub fn contents(&self) -> &SecretString {
        &self.contents
    }
    pub fn mode(&self) -> u32 {
        self.mode
    }
}

/// A pure, executable launch description.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchPlan {
    program: PathBuf,
    argv: Vec<String>,
    env: Vec<LaunchEnv>,
    url: Option<String>,
    temp_file: Option<TempFile>,
    command_string: Option<String>,
    required_path: Option<PathBuf>,
    copy_uri_hint: Option<String>,
}

impl LaunchPlan {
    pub fn new(
        program: impl Into<PathBuf>,
        argv: Vec<String>,
        env: Vec<LaunchEnv>,
        url: Option<String>,
        temp_file: Option<TempFile>,
        command_string: Option<String>,
    ) -> Self {
        Self {
            program: program.into(),
            argv,
            env,
            url,
            temp_file,
            command_string,
            required_path: None,
            copy_uri_hint: None,
        }
    }

    pub fn with_required_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.required_path = Some(path.into());
        self
    }

    pub fn with_copy_uri_hint(mut self, uri: impl Into<String>) -> Self {
        self.copy_uri_hint = Some(uri.into());
        self
    }

    pub fn program(&self) -> &PathBuf {
        &self.program
    }
    pub fn argv(&self) -> &[String] {
        &self.argv
    }
    pub fn env(&self) -> &[LaunchEnv] {
        &self.env
    }
    pub fn url(&self) -> Option<&str> {
        self.url.as_deref()
    }
    pub fn temp_file(&self) -> Option<&TempFile> {
        self.temp_file.as_ref()
    }
    pub fn command_string(&self) -> Option<&str> {
        self.command_string.as_deref()
    }
    pub fn required_path(&self) -> Option<&PathBuf> {
        self.required_path.as_ref()
    }
    pub fn copy_uri_hint(&self) -> Option<&str> {
        self.copy_uri_hint.as_deref()
    }
}

/// Plans how to open `client` against `target`.
pub fn plan_open(
    client: &DetectedClient,
    target: &OpenTarget,
    credential: Option<&SecretString>,
    os: OsKind,
    terminal: &TerminalChoice,
) -> ClientResult<LaunchPlan> {
    if !client.supports(target.engine) {
        return Err(ClientError::UnsupportedClient {
            client: client.kind().as_str(),
            engine: target.engine.as_str(),
        });
    }
    match client.kind() {
        ClientKind::TablePlus => plan_tableplus(client, target, os),
        ClientKind::DBeaver => plan_dbeaver(client, target),
        ClientKind::DbDelve => plan_dbdelve(client, target),
        ClientKind::Psql
        | ClientKind::Mysql
        | ClientKind::Mariadb
        | ClientKind::ValkeyCli
        | ClientKind::RedisCli => plan_cli(client, target, credential, os, terminal),
    }
}

fn plan_tableplus(
    client: &DetectedClient,
    target: &OpenTarget,
    os: OsKind,
) -> ClientResult<LaunchPlan> {
    // Official TablePlus docs: open with connection URL form, password omitted.
    // https://docs.tableplus.com/gui-tools/manage-connections
    let url = tableplus_url(target)?;
    let (program, argv) = match os {
        OsKind::Macos => (
            "open".to_owned(),
            vec!["-a".to_owned(), "TablePlus".to_owned(), url.clone()],
        ),
        OsKind::Linux | OsKind::Windows => (
            client.path().to_string_lossy().into_owned(),
            vec![url.clone()],
        ),
    };
    Ok(
        LaunchPlan::new(program, argv, vec![], Some(url), None, None)
            .with_required_path(client.path().clone()),
    )
}

fn tableplus_url(target: &OpenTarget) -> ClientResult<String> {
    let user = encode_component(target.username());
    let host = encode_component(target.host());
    let db = encode_component(target.database());
    let url = match target.engine {
        EngineKey::Postgresql => format!("postgresql://{user}@{host}:{}/{db}", target.port()),
        EngineKey::Mysql | EngineKey::Mariadb => {
            format!("mysql://{user}@{host}:{}/{db}", target.port())
        }
        EngineKey::Valkey => format!("redis://{user}@{host}:{}/{db}", target.port()),
    };
    Ok(url)
}

fn plan_dbeaver(client: &DetectedClient, target: &OpenTarget) -> ClientResult<LaunchPlan> {
    let driver = match target.engine {
        EngineKey::Postgresql => "postgresql",
        EngineKey::Mysql => "mysql",
        EngineKey::Mariadb => "maria",
        EngineKey::Valkey => "redis",
    };
    let spec = format!(
        "driver={driver}|name={}|host={}|port={}|user={}|database={}|openConsole=true",
        sanitize_connection_name(target.instance_name()),
        sanitize_con_value(target.host()),
        target.port(),
        sanitize_con_value(target.username()),
        sanitize_con_value(target.database()),
    );
    Ok(LaunchPlan::new(
        client.path().clone(),
        vec!["-con".to_owned(), spec],
        vec![],
        None,
        None,
        None,
    )
    .with_required_path(client.path().clone()))
}

fn plan_dbdelve(client: &DetectedClient, target: &OpenTarget) -> ClientResult<LaunchPlan> {
    if matches!(target.engine, EngineKey::Valkey) {
        return Err(ClientError::UnsupportedClient {
            client: ClientKind::DbDelve.as_str(),
            engine: EngineKey::Valkey.as_str(),
        });
    }
    // DBDelve connection-argument mechanism is unverified on this machine;
    // launch the app only and let the GUI copy a password-less URI.
    let uri = passwordless_uri(target)?;
    Ok(
        LaunchPlan::new(client.path().clone(), vec![], vec![], None, None, None)
            .with_required_path(client.path().clone())
            .with_copy_uri_hint(uri),
    )
}

fn passwordless_uri(target: &OpenTarget) -> ClientResult<String> {
    let user = encode_component(target.username());
    let host = encode_component(target.host());
    let db = encode_component(target.database());
    let url = match target.engine {
        EngineKey::Postgresql => format!("postgresql://{user}@{host}:{}/{db}", target.port()),
        EngineKey::Mysql | EngineKey::Mariadb => {
            format!("mysql://{user}@{host}:{}/{db}", target.port())
        }
        EngineKey::Valkey => format!("redis://{user}@{host}:{}/{db}", target.port()),
    };
    Ok(url)
}

fn plan_cli(
    client: &DetectedClient,
    target: &OpenTarget,
    credential: Option<&SecretString>,
    os: OsKind,
    terminal: &TerminalChoice,
) -> ClientResult<LaunchPlan> {
    let (client_program, client_argv, env) = cli_command(client, target, credential)?;
    build_terminal_plan(client_program, client_argv, env, os, terminal)
        .map(|plan| plan.with_required_path(client.path().clone()))
}

fn cli_command(
    client: &DetectedClient,
    target: &OpenTarget,
    credential: Option<&SecretString>,
) -> ClientResult<(PathBuf, Vec<String>, Vec<LaunchEnv>)> {
    let kind = client.kind();
    let mut env = Vec::new();
    if let Some(secret) = credential
        && let Some(name) = password_env_name(kind)
    {
        env.push(LaunchEnv::new(name, secret.clone()));
    }
    let program = client.path().clone();
    let argv = match kind {
        ClientKind::Psql => vec![psql_conninfo(target)],
        ClientKind::Mysql | ClientKind::Mariadb => {
            let mut argv = vec![
                format!("--host={}", target.host()),
                format!("--port={}", target.port()),
                format!("--user={}", target.username()),
                "--protocol=tcp".to_owned(),
            ];
            if !target.database().is_empty() {
                argv.push(format!("--database={}", target.database()));
            }
            argv
        }
        ClientKind::ValkeyCli | ClientKind::RedisCli => {
            let mut argv = vec![
                "-h".to_owned(),
                target.host().to_owned(),
                "-p".to_owned(),
                target.port().to_string(),
            ];
            if !target.username().is_empty() && target.username() != "default" {
                argv.push("--user".to_owned());
                argv.push(target.username().to_owned());
            }
            argv
        }
        _ => {
            return Err(ClientError::UnsupportedClient {
                client: kind.as_str(),
                engine: "unknown",
            });
        }
    };
    Ok((program, argv, env))
}

fn psql_conninfo(target: &OpenTarget) -> String {
    // Use URI form so dbname is never parsed as libpq keyword conninfo.
    // Password is intentionally omitted; psql reads PGPASSWORD from env.
    let user = encode_component(target.username());
    let host = encode_component(target.host());
    let db = encode_component(target.database());
    format!("postgresql://{user}@{host}:{}/{}", target.port(), db)
}
fn password_env_name(kind: ClientKind) -> Option<&'static str> {
    match kind {
        ClientKind::Psql => Some("PGPASSWORD"),
        ClientKind::Mysql | ClientKind::Mariadb => Some("MYSQL_PWD"),
        ClientKind::ValkeyCli | ClientKind::RedisCli => Some("REDISCLI_AUTH"),
        _ => None,
    }
}

fn build_terminal_plan(
    client_program: PathBuf,
    client_argv: Vec<String>,
    env: Vec<LaunchEnv>,
    os: OsKind,
    terminal: &TerminalChoice,
) -> ClientResult<LaunchPlan> {
    match os {
        OsKind::Linux => build_linux_terminal_plan(client_program, client_argv, env, terminal),
        OsKind::Macos => build_macos_terminal_plan(client_program, client_argv, env),
        OsKind::Windows => build_windows_terminal_plan(client_program, client_argv, env, terminal),
    }
}

fn build_linux_terminal_plan(
    client_program: PathBuf,
    client_argv: Vec<String>,
    env: Vec<LaunchEnv>,
    terminal: &TerminalChoice,
) -> ClientResult<LaunchPlan> {
    let (argv, temp_file, command_string) = if env.is_empty() {
        let mut argv = Vec::new();
        push_linux_exec_argv(&mut argv, terminal.kind(), &client_program, &client_argv);
        (argv, None, None)
    } else {
        let temp_file = launcher_temp_file(&client_program, &client_argv, &env, OsKind::Linux)?;
        let cmd = format!("sh {}", shell_quote(SCRIPT_PLACEHOLDER));
        let mut argv = Vec::new();
        push_linux_script_argv(&mut argv, terminal.kind());
        (argv, Some(temp_file), Some(cmd))
    };
    Ok(LaunchPlan::new(
        terminal.program().clone(),
        argv,
        vec![],
        None,
        temp_file,
        command_string,
    ))
}

fn build_macos_terminal_plan(
    client_program: PathBuf,
    client_argv: Vec<String>,
    env: Vec<LaunchEnv>,
) -> ClientResult<LaunchPlan> {
    let temp_file = launcher_temp_file(&client_program, &client_argv, &env, OsKind::Macos)?;
    let command_string = format!("sh {}", shell_quote(SCRIPT_PLACEHOLDER));
    let apple_script = format!(
        "tell application \"Terminal\" to do script \"{}\"",
        escape_applescript(&command_string)
    );
    Ok(LaunchPlan::new(
        "osascript",
        vec!["-e".to_owned(), apple_script],
        vec![],
        None,
        Some(temp_file),
        Some(command_string),
    ))
}

fn build_windows_terminal_plan(
    client_program: PathBuf,
    client_argv: Vec<String>,
    env: Vec<LaunchEnv>,
    terminal: &TerminalChoice,
) -> ClientResult<LaunchPlan> {
    let temp_file = launcher_temp_file(&client_program, &client_argv, &env, OsKind::Windows)?;
    let script_path = SCRIPT_PLACEHOLDER.to_owned();
    let argv = match terminal.kind() {
        TerminalKind::WindowsTerminal => vec!["cmd.exe".to_owned(), "/c".to_owned(), script_path],
        _ => vec!["/c".to_owned(), script_path],
    };
    Ok(LaunchPlan::new(
        terminal.program().clone(),
        argv,
        vec![],
        None,
        Some(temp_file),
        None,
    ))
}

fn push_linux_exec_argv(
    argv: &mut Vec<String>,
    kind: TerminalKind,
    program: &Path,
    client_argv: &[String],
) {
    let prog = program.to_string_lossy().into_owned();
    match kind {
        TerminalKind::GnomeTerminal
        | TerminalKind::Ptyxis
        | TerminalKind::Wezterm
        | TerminalKind::XdgTerminalExec => {
            argv.push("--".to_owned());
            argv.push(prog);
            argv.extend(client_argv.iter().cloned());
        }
        TerminalKind::Kitty => {
            argv.push("--".to_owned());
            argv.push(prog);
            argv.extend(client_argv.iter().cloned());
        }
        _ => {
            argv.push("-e".to_owned());
            argv.push(prog);
            argv.extend(client_argv.iter().cloned());
        }
    }
}

fn push_linux_script_argv(argv: &mut Vec<String>, kind: TerminalKind) {
    let p = SCRIPT_PLACEHOLDER;
    match kind {
        TerminalKind::Ptyxis => {
            argv.push("--new-window".to_owned());
            argv.push("--".to_owned());
            argv.push("sh".to_owned());
            argv.push(p.to_owned());
        }
        TerminalKind::Wezterm => {
            argv.push("start".to_owned());
            argv.push("--".to_owned());
            argv.push("sh".to_owned());
            argv.push(p.to_owned());
        }
        TerminalKind::XdgTerminalExec | TerminalKind::GnomeTerminal => {
            argv.push("--".to_owned());
            argv.push("sh".to_owned());
            argv.push(p.to_owned());
        }
        TerminalKind::Foot => {
            argv.push("sh".to_owned());
            argv.push(p.to_owned());
        }
        TerminalKind::Kitty => {
            // kitty takes command as positional args after options; `--` separates options.
            argv.push("--".to_owned());
            argv.push("sh".to_owned());
            argv.push(p.to_owned());
        }
        _ => {
            argv.push("-e".to_owned());
            argv.push("sh".to_owned());
            argv.push(p.to_owned());
        }
    }
}

fn encode_component(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for b in value.bytes() {
        let safe = b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~');
        if safe {
            out.push(char::from(b));
        } else {
            out.push('%');
            out.push(hex_upper(b >> 4));
            out.push(hex_upper(b & 0x0f));
        }
    }
    out
}

fn hex_upper(nibble: u8) -> char {
    match nibble {
        0..=9 => char::from(b'0' + nibble),
        10..=15 => char::from(b'A' + (nibble - 10)),
        _ => '0',
    }
}

fn sanitize_connection_name(value: &str) -> String {
    let mut out: String = value
        .chars()
        .map(|c| if c.is_control() || c == '|' { '_' } else { c })
        .collect();
    if out.is_empty() {
        out.push_str("dblocal");
    }
    out
}

fn sanitize_con_value(value: &str) -> String {
    value
        .chars()
        .map(|c| if c.is_control() || c == '|' { '_' } else { c })
        .collect()
}
