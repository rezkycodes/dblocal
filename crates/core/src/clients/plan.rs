//! Pure launch planning (F-OPN-02).

use std::path::PathBuf;

use crate::error::{DblError, Result};
use crate::model::EngineKey;
use crate::secret::SecretString;

use super::detect::{ClientKind, DetectedClient, HostProbe};
use super::quote::shell_quote;
use super::terminal::{
    escape_applescript, launcher_temp_file, macos_launcher_script_path,
    windows_launcher_script_path,
};

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
    /// Creates an open target. Values are treated as untrusted input.
    pub fn new(
        engine: EngineKey,
        host: impl Into<String>,
        port: u16,
        username: impl Into<String>,
        database: impl Into<String>,
        instance_name: impl Into<String>,
    ) -> Self {
        Self {
            engine,
            host: host.into(),
            port,
            username: username.into(),
            database: database.into(),
            instance_name: instance_name.into(),
        }
    }

    pub fn engine(&self) -> EngineKey { self.engine }
    pub fn host(&self) -> &str { &self.host }
    pub fn port(&self) -> u16 { self.port }
    pub fn username(&self) -> &str { &self.username }
    pub fn database(&self) -> &str { &self.database }
    pub fn instance_name(&self) -> &str { &self.instance_name }
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
        if cfg!(target_os = "macos") { Self::Macos }
        else if cfg!(target_os = "windows") { Self::Windows }
        else { Self::Linux }
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
        Self { program: program.into(), kind }
    }
    pub fn program(&self) -> &PathBuf { &self.program }
    pub fn kind(&self) -> TerminalKind { self.kind }
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
        Self { name: name.into(), value }
    }
    pub fn name(&self) -> &str { &self.name }
    pub fn value(&self) -> &SecretString { &self.value }
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
        Self { name_hint: name_hint.into(), contents, mode }
    }
    pub fn name_hint(&self) -> &str { &self.name_hint }
    pub fn contents(&self) -> &SecretString { &self.contents }
    pub fn mode(&self) -> u32 { self.mode }
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
        Self { program: program.into(), argv, env, url, temp_file, command_string }
    }
    pub fn program(&self) -> &PathBuf { &self.program }
    pub fn argv(&self) -> &[String] { &self.argv }
    pub fn env(&self) -> &[LaunchEnv] { &self.env }
    pub fn url(&self) -> Option<&str> { self.url.as_deref() }
    pub fn temp_file(&self) -> Option<&TempFile> { self.temp_file.as_ref() }
    pub fn command_string(&self) -> Option<&str> { self.command_string.as_deref() }
}

/// Plans how to open `client` against `target`.
pub fn plan_open(
    client: &DetectedClient,
    target: &OpenTarget,
    credential: Option<&SecretString>,
    os: OsKind,
    terminal: &TerminalChoice,
) -> Result<LaunchPlan> {
    if !client.supports(target.engine) {
        return Err(DblError::Unsupported {
            reason: format!(
                "{} does not support {}",
                client.kind().display_name(),
                target.engine.display_name()
            ),
        });
    }
    match client.kind() {
        ClientKind::TablePlus => plan_tableplus(target),
        ClientKind::DBeaver => plan_dbeaver(client, target),
        ClientKind::DbDelve => plan_dbdelve(client, target),
        ClientKind::Psql | ClientKind::Mysql | ClientKind::Mariadb
        | ClientKind::ValkeyCli | ClientKind::RedisCli => {
            plan_cli(client, target, credential, os, terminal)
        }
    }
}

fn plan_tableplus(target: &OpenTarget) -> Result<LaunchPlan> {
    let driver = match target.engine {
        EngineKey::Postgresql => "postgresql",
        EngineKey::Mysql | EngineKey::Mariadb => "mysql",
        EngineKey::Valkey => "redis",
    };
    let url = format!(
        "tableplus://?name={}&driver={driver}&host={}&port={}&user={}&database={}",
        encode_component(target.instance_name()),
        encode_component(target.host()),
        target.port(),
        encode_component(target.username()),
        encode_component(target.database()),
    );
    Ok(LaunchPlan::new("xdg-open", vec![url.clone()], vec![], Some(url), None, None))
}

fn plan_dbeaver(client: &DetectedClient, target: &OpenTarget) -> Result<LaunchPlan> {
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
    Ok(LaunchPlan::new(client.path().clone(), vec!["-con".to_owned(), spec], vec![], None, None, None))
}

fn plan_dbdelve(client: &DetectedClient, target: &OpenTarget) -> Result<LaunchPlan> {
    let driver = match target.engine {
        EngineKey::Postgresql => "postgresql",
        EngineKey::Mysql | EngineKey::Mariadb => "mysql",
        EngineKey::Valkey => {
            return Err(DblError::Unsupported {
                reason: "DBDelve does not support Valkey".to_owned(),
            });
        }
    };
    Ok(LaunchPlan::new(
        client.path().clone(),
        vec![
            "--driver".to_owned(), driver.to_owned(),
            "--host".to_owned(), target.host().to_owned(),
            "--port".to_owned(), target.port().to_string(),
            "--user".to_owned(), target.username().to_owned(),
            "--database".to_owned(), target.database().to_owned(),
            "--name".to_owned(), target.instance_name().to_owned(),
        ],
        vec![], None, None, None,
    ))
}

fn plan_cli(
    client: &DetectedClient,
    target: &OpenTarget,
    credential: Option<&SecretString>,
    os: OsKind,
    terminal: &TerminalChoice,
) -> Result<LaunchPlan> {
    let (client_program, client_argv, env) = cli_command(client.kind(), target, credential)?;
    build_terminal_plan(client_program, client_argv, env, os, terminal)
}

fn cli_command(
    kind: ClientKind,
    target: &OpenTarget,
    credential: Option<&SecretString>,
) -> Result<(PathBuf, Vec<String>, Vec<LaunchEnv>)> {
    let mut env = Vec::new();
    if let Some(secret) = credential {
        if let Some(name) = password_env_name(kind) {
            env.push(LaunchEnv::new(name, secret.clone()));
        }
    }
    let (program, argv) = match kind {
        ClientKind::Psql => (PathBuf::from("psql"), vec![
            "--host".to_owned(), target.host().to_owned(),
            "--port".to_owned(), target.port().to_string(),
            "--username".to_owned(), target.username().to_owned(),
            "--dbname".to_owned(), target.database().to_owned(),
        ]),
        ClientKind::Mysql | ClientKind::Mariadb => {
            let bin = if kind == ClientKind::Mysql { "mysql" } else { "mariadb" };
            (PathBuf::from(bin), vec![
                "--host".to_owned(), target.host().to_owned(),
                "--port".to_owned(), target.port().to_string(),
                "--user".to_owned(), target.username().to_owned(),
                "--protocol".to_owned(), "tcp".to_owned(),
                target.database().to_owned(),
            ])
        }
        ClientKind::ValkeyCli | ClientKind::RedisCli => {
            let bin = if kind == ClientKind::ValkeyCli { "valkey-cli" } else { "redis-cli" };
            let mut argv = vec![
                "-h".to_owned(), target.host().to_owned(),
                "-p".to_owned(), target.port().to_string(),
            ];
            if !target.username().is_empty() && target.username() != "default" {
                argv.push("--user".to_owned());
                argv.push(target.username().to_owned());
            }
            (PathBuf::from(bin), argv)
        }
        _ => return Err(DblError::Unsupported { reason: format!("{} is not a CLI client", kind.display_name()) }),
    };
    Ok((program, argv, env))
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
) -> Result<LaunchPlan> {
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
) -> Result<LaunchPlan> {
    let mut argv = Vec::new();
    match terminal.kind() {
        TerminalKind::GnomeTerminal => {
            argv.push("--".to_owned());
            argv.push(client_program.to_string_lossy().into_owned());
            argv.extend(client_argv);
        }
        _ => {
            argv.push("-e".to_owned());
            argv.push(client_program.to_string_lossy().into_owned());
            argv.extend(client_argv);
        }
    }
    Ok(LaunchPlan::new(terminal.program().clone(), argv, env, None, None, None))
}

fn build_macos_terminal_plan(
    client_program: PathBuf,
    client_argv: Vec<String>,
    env: Vec<LaunchEnv>,
) -> Result<LaunchPlan> {
    let temp_file = launcher_temp_file(&client_program, &client_argv, &env, OsKind::Macos);
    let script_path = macos_launcher_script_path(temp_file.name_hint());
    let command_string = format!("sh {}", shell_quote(&script_path));
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
) -> Result<LaunchPlan> {
    let temp_file = launcher_temp_file(&client_program, &client_argv, &env, OsKind::Windows);
    let script_path = windows_launcher_script_path(temp_file.name_hint());
    let argv = match terminal.kind() {
        TerminalKind::WindowsTerminal => vec!["cmd.exe".to_owned(), "/c".to_owned(), script_path],
        _ => vec!["/c".to_owned(), script_path],
    };
    Ok(LaunchPlan::new(terminal.program().clone(), argv, vec![], None, Some(temp_file), None))
}

fn encode_component(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for b in value.bytes() {
        let safe = b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~');
        if safe { out.push(char::from(b)); }
        else {
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
    let mut out: String = value.chars().map(|c| if c.is_control() || c == '|' { '_' } else { c }).collect();
    if out.is_empty() { out.push_str("dblocal"); }
    out
}

fn sanitize_con_value(value: &str) -> String {
    value.chars().map(|c| if c.is_control() || c == '|' { '_' } else { c }).collect()
}
