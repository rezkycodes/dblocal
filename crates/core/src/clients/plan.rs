//! Pure launch planning (F-OPN-02).
//!
//! [`plan_open`] builds a [`LaunchPlan`] without side effects. The plan never
//! contains the password in argv, URL, or command string; secrets are carried in
//! [`LaunchEnv`] for the spawner to apply to the child process environment, or in
//! a private [`TempFile`] launcher script when a terminal cannot safely inherit
//! environment variables.

use std::path::PathBuf;

use crate::error::{DblError, Result};
use crate::model::EngineKey;
use crate::secret::SecretString;

use super::detect::{ClientKind, DetectedClient, HostProbe};
use super::quote::shell_quote;

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

    /// Engine kind.
    pub fn engine(&self) -> EngineKey {
        self.engine
    }

    /// Hostname, normally `127.0.0.1`.
    pub fn host(&self) -> &str {
        &self.host
    }

    /// TCP port.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Database username.
    pub fn username(&self) -> &str {
        &self.username
    }

    /// Database name.
    pub fn database(&self) -> &str {
        &self.database
    }

    /// Instance name, for display/terminal titles.
    pub fn instance_name(&self) -> &str {
        &self.instance_name
    }
}

/// OS family for deterministic plan generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OsKind {
    /// Linux.
    Linux,
    /// macOS.
    Macos,
    /// Windows.
    Windows,
}

impl OsKind {
    /// Detects the current OS.
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
    /// Creates a terminal choice.
    pub fn new(program: impl Into<PathBuf>, kind: TerminalKind) -> Self {
        Self {
            program: program.into(),
            kind,
        }
    }

    /// Terminal executable.
    pub fn program(&self) -> &PathBuf {
        &self.program
    }

    /// Terminal kind.
    pub fn kind(&self) -> TerminalKind {
        self.kind
    }
}

/// Supported terminal emulator kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TerminalKind {
    /// `$TERMINAL` or generic `-e <argv...>` terminal.
    GenericArgv,
    /// `gnome-terminal -- <argv...>`
    GnomeTerminal,
    /// `konsole -e <argv...>`
    Konsole,
    /// `kitty -- <argv...>`
    Kitty,
    /// `alacritty -e <argv...>`
    Alacritty,
    /// `xterm -e <argv...>`
    Xterm,
    /// macOS Terminal.app via `open -a Terminal <script>`.
    MacOsTerminalApp,
    /// Windows Terminal.
    WindowsTerminal,
    /// Windows cmd.exe fallback.
    WindowsCmd,
}

impl TerminalKind {
    /// Whether this terminal can safely inherit env vars through process spawn
    /// without embedding them in a command string.
    pub const fn supports_env_inheritance(self) -> bool {
        matches!(
            self,
            Self::GenericArgv
                | Self::GnomeTerminal
                | Self::Konsole
                | Self::Kitty
                | Self::Alacritty
                | Self::Xterm
        )
    }
}

/// Resolves the terminal to use for CLI clients on the given OS.
pub fn resolve_terminal(host: &dyn HostProbe, os: OsKind) -> TerminalChoice {
    match os {
        OsKind::Linux => resolve_linux_terminal(host),
        OsKind::Macos => TerminalChoice::new("open", TerminalKind::MacOsTerminalApp),
        OsKind::Windows => resolve_windows_terminal(host),
    }
}

fn resolve_linux_terminal(host: &dyn HostProbe) -> TerminalChoice {
    if let Some(path) = std::env::var_os("TERMINAL") {
        let path = PathBuf::from(path);
        if !path.as_os_str().is_empty() {
            return TerminalChoice::new(path, TerminalKind::GenericArgv);
        }
    }
    for (name, kind) in [
        ("x-terminal-emulator", TerminalKind::GenericArgv),
        ("gnome-terminal", TerminalKind::GnomeTerminal),
        ("konsole", TerminalKind::Konsole),
        ("kitty", TerminalKind::Kitty),
        ("alacritty", TerminalKind::Alacritty),
        ("xterm", TerminalKind::Xterm),
    ] {
        if let Some(path) = host.which(name) {
            return TerminalChoice::new(path, kind);
        }
    }
    TerminalChoice::new("xterm", TerminalKind::Xterm)
}

fn resolve_windows_terminal(host: &dyn HostProbe) -> TerminalChoice {
    if let Some(path) = host.which("wt.exe").or_else(|| host.which("wt")) {
        return TerminalChoice::new(path, TerminalKind::WindowsTerminal);
    }
    TerminalChoice::new("cmd.exe", TerminalKind::WindowsCmd)
}

/// Environment variable to set on the spawned process.
///
/// The value is secret when it carries a password.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchEnv {
    name: String,
    value: SecretString,
}

impl LaunchEnv {
    /// Creates an env var.
    pub fn new(name: impl Into<String>, value: SecretString) -> Self {
        Self {
            name: name.into(),
            value,
        }
    }

    /// Variable name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Variable value (secret).
    pub fn value(&self) -> &SecretString {
        &self.value
    }
}

/// A private temporary file the spawner should create before launch.
///
/// Used for launcher scripts when a terminal cannot safely inherit env vars.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TempFile {
    name_hint: String,
    contents: SecretString,
    mode: u32,
}

impl TempFile {
    /// Creates a temp file description.
    pub fn new(name_hint: impl Into<String>, contents: SecretString, mode: u32) -> Self {
        Self {
            name_hint: name_hint.into(),
            contents,
            mode,
        }
    }

    /// File name hint.
    pub fn name_hint(&self) -> &str {
        &self.name_hint
    }

    /// File contents. Secret because it may contain a password.
    pub fn contents(&self) -> &SecretString {
        &self.contents
    }

    /// Unix mode.
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
}

impl LaunchPlan {
    /// Creates a launch plan. Intended for tests and plan builders.
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
        }
    }

    /// Program to execute.
    pub fn program(&self) -> &PathBuf {
        &self.program
    }

    /// Arguments, excluding argv[0].
    pub fn argv(&self) -> &[String] {
        &self.argv
    }

    /// Environment variables for the spawned process.
    pub fn env(&self) -> &[LaunchEnv] {
        &self.env
    }

    /// Optional URL to open (GUI clients).
    pub fn url(&self) -> Option<&str> {
        self.url.as_deref()
    }

    /// Optional private temp file to create before launch.
    pub fn temp_file(&self) -> Option<&TempFile> {
        self.temp_file.as_ref()
    }

    /// Optional shell command string used by some terminal forms.
    ///
    /// This is secret-bearing only through `temp_file`; it must never contain a
    /// password.
    pub fn command_string(&self) -> Option<&str> {
        self.command_string.as_deref()
    }
}

/// Plans how to open `client` against `target`.
///
/// Pure: no filesystem, no spawning, no host probing. Terminal choice and OS are
/// supplied by the caller.
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
    if !client.path().as_os_str().is_empty() {
        // Path is proof of detection; disappearance is handled by run/spawner.
    }
    match client.kind() {
        ClientKind::TablePlus => plan_tableplus(target),
        ClientKind::DBeaver => plan_dbeaver(client, target),
        ClientKind::DbDelve => plan_dbdelve(client, target),
        ClientKind::Psql
        | ClientKind::Mysql
        | ClientKind::Mariadb
        | ClientKind::ValkeyCli
        | ClientKind::RedisCli => plan_cli(client, target, credential, os, terminal),
    }
}

fn plan_tableplus(target: &OpenTarget) -> Result<LaunchPlan> {
    // TablePlus URL scheme is documented. Password is intentionally omitted:
    // embedding it would leak in process listings. TablePlus prompts or uses
    // its own saved connection.
    let driver = match target.engine {
        EngineKey::Postgresql => "postgresql",
        EngineKey::Mysql | EngineKey::Mariadb => "mysql",
        EngineKey::Valkey => "redis",
    };
    let name = encode_component(target.instance_name());
    let host = encode_component(target.host());
    let user = encode_component(target.username());
    let db = encode_component(target.database());
    let url = format!(
        "tableplus://?name={name}&driver={driver}&host={host}&port={}&user={user}&database={db}",
        target.port()
    );
    Ok(LaunchPlan {
        program: PathBuf::from("xdg-open"),
        argv: vec![url.clone()],
        env: Vec::new(),
        url: Some(url),
        temp_file: None,
        command_string: None,
    })
}

fn plan_dbeaver(client: &DetectedClient, target: &OpenTarget) -> Result<LaunchPlan> {
    // DBeaver -con takes a connection spec. We avoid embedding password. The
    // exact -con format varies; we pass driver and connection properties as a
    // conservative argument and document verification limits.
    let driver = match target.engine {
        EngineKey::Postgresql => "postgresql",
        EngineKey::Mysql => "mysql",
        EngineKey::Mariadb => "maria",
        EngineKey::Valkey => "redis",
    };
    let name = sanitize_connection_name(target.instance_name());
    let spec = format!(
        "driver={driver}|name={name}|host={}|port={}|user={}|database={}|openConsole=true",
        sanitize_con_value(target.host()),
        target.port(),
        sanitize_con_value(target.username()),
        sanitize_con_value(target.database()),
    );
    Ok(LaunchPlan {
        program: client.path().clone(),
        argv: vec!["-con".to_owned(), spec],
        env: Vec::new(),
        url: None,
        temp_file: None,
        command_string: None,
    })
}

fn plan_dbdelve(client: &DetectedClient, target: &OpenTarget) -> Result<LaunchPlan> {
    // DBDelve: pass connection details without password. We use argv, not a
    // shell string. Password omitted; DBDelve should prompt.
    let driver = match target.engine {
        EngineKey::Postgresql => "postgresql",
        EngineKey::Mysql | EngineKey::Mariadb => "mysql",
        EngineKey::Valkey => {
            return Err(DblError::Unsupported {
                reason: "DBDelve does not support Valkey".to_owned(),
            });
        }
    };
    Ok(LaunchPlan {
        program: client.path().clone(),
        argv: vec![
            "--driver".to_owned(),
            driver.to_owned(),
            "--host".to_owned(),
            target.host().to_owned(),
            "--port".to_owned(),
            target.port().to_string(),
            "--user".to_owned(),
            target.username().to_owned(),
            "--database".to_owned(),
            target.database().to_owned(),
            "--name".to_owned(),
            target.instance_name().to_owned(),
        ],
        env: Vec::new(),
        url: None,
        temp_file: None,
        command_string: None,
    })
}

fn plan_cli(
    client: &DetectedClient,
    target: &OpenTarget,
    credential: Option<&SecretString>,
    os: OsKind,
    terminal: &TerminalChoice,
) -> Result<LaunchPlan> {
    let (client_program, client_argv, env) = cli_command(client.kind(), target, credential)?;
    build_terminal_plan(client_program, client_argv, env, os, terminal, target)
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
        ClientKind::Psql => (
            PathBuf::from("psql"),
            vec![
                "--host".to_owned(),
                target.host().to_owned(),
                "--port".to_owned(),
                target.port().to_string(),
                "--username".to_owned(),
                target.username().to_owned(),
                "--dbname".to_owned(),
                target.database().to_owned(),
            ],
        ),
        ClientKind::Mysql | ClientKind::Mariadb => {
            let bin = if kind == ClientKind::Mysql {
                "mysql"
            } else {
                "mariadb"
            };
            (
                PathBuf::from(bin),
                vec![
                    "--host".to_owned(),
                    target.host().to_owned(),
                    "--port".to_owned(),
                    target.port().to_string(),
                    "--user".to_owned(),
                    target.username().to_owned(),
                    "--protocol".to_owned(),
                    "tcp".to_owned(),
                    target.database().to_owned(),
                ],
            )
        }
        ClientKind::ValkeyCli | ClientKind::RedisCli => {
            let bin = if kind == ClientKind::ValkeyCli {
                "valkey-cli"
            } else {
                "redis-cli"
            };
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
            (
                PathBuf::from(bin),
                argv,
            )
        }
        _ => {
            return Err(DblError::Unsupported {
                reason: format!("{} is not a CLI client", kind.display_name()),
            });
        }
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
    target: &OpenTarget,
) -> Result<LaunchPlan> {
    match os {
        OsKind::Linux => build_linux_terminal_plan(client_program, client_argv, env, terminal),
        OsKind::Macos => build_macos_terminal_plan(client_program, client_argv, env, target),
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
        TerminalKind::Konsole
        | TerminalKind::Alacritty
        | TerminalKind::Xterm
        | TerminalKind::GenericArgv => {
            argv.push("-e".to_owned());
            argv.push(client_program.to_string_lossy().into_owned());
            argv.extend(client_argv);
        }
        _ => {
            argv.push("-e".to_owned());
            argv.push(client_program.to_string_lossy().into_owned());
            argv.extend(client_argv);
        }
    }
    Ok(LaunchPlan {
        program: terminal.program().clone(),
        argv,
        env,
        url: None,
        temp_file: None,
        command_string: None,
    })
}

fn build_macos_terminal_plan(
    client_program: PathBuf,
    client_argv: Vec<String>,
    env: Vec<LaunchEnv>,
    _target: &OpenTarget,
) -> Result<LaunchPlan> {
    // macOS Terminal.app cannot safely inherit custom env vars through `open`.
    // We use a private launcher script. The script exports env vars, deletes
    // itself first, then execs the client. The script is secret-bearing.
    let script = build_launcher_script(&client_program, &client_argv, &env, OsKind::Macos);
    let temp_file = TempFile::new("dblocal-open-client.sh", script, 0o600);
    let script_path = launcher_script_path("dblocal-open-client.sh");
    let command_string = format!("sh {}", shell_quote(&script_path));
    let apple_script = format!(
        "tell application \"Terminal\" to do script \"{}\"",
        escape_applescript(&command_string)
    );
    Ok(LaunchPlan {
        program: PathBuf::from("osascript"),
        argv: vec![
            "-e".to_owned(),
            apple_script,
            // keep instance name out of command execution; no-op metadata is not needed
        ],
        env: Vec::new(),
        url: None,
        temp_file: Some(temp_file),
        command_string: Some(command_string),
    })
}

fn build_windows_terminal_plan(
    client_program: PathBuf,
    client_argv: Vec<String>,
    env: Vec<LaunchEnv>,
    terminal: &TerminalChoice,
) -> Result<LaunchPlan> {
    // Windows terminal/cmd command lines are visible in process listings in some
    // contexts and env inheritance through command strings is unsafe. Use a
    // private .cmd launcher script.
    let script = build_launcher_script(&client_program, &client_argv, &env, OsKind::Windows);
    let temp_file = TempFile::new("dblocal-open-client.cmd", script, 0o600);
    let script_path = windows_launcher_script_path("dblocal-open-client.cmd");
    match terminal.kind() {
        TerminalKind::WindowsTerminal => Ok(LaunchPlan {
            program: terminal.program().clone(),
            argv: vec!["cmd.exe".to_owned(), "/c".to_owned(), script_path],
            env: Vec::new(),
            url: None,
            temp_file: Some(temp_file),
            command_string: None,
        }),
        _ => Ok(LaunchPlan {
            program: terminal.program().clone(),
            argv: vec!["/c".to_owned(), script_path],
            env: Vec::new(),
            url: None,
            temp_file: Some(temp_file),
            command_string: None,
        }),
    }
}

fn build_launcher_script(
    client_program: &std::path::Path,
    client_argv: &[String],
    env: &[LaunchEnv],
    os: OsKind,
) -> SecretString {
    match os {
        OsKind::Windows => SecretString::new(build_windows_launcher(client_program, client_argv, env)),
        _ => SecretString::new(build_posix_launcher(client_program, client_argv, env)),
    }
}

fn build_posix_launcher(
    client_program: &std::path::Path,
    client_argv: &[String],
    env: &[LaunchEnv],
) -> String {
    // Self-remove is first real action: the script contains secrets.
    let mut out = String::from("#!/bin/sh\nset -eu\nrm -f -- \"$0\"\n");
    for var in env {
        out.push_str("export ");
        out.push_str(var.name());
        out.push('=');
        out.push_str(&shell_quote(var.value().expose()));
        out.push('\n');
    }
    out.push_str("exec ");
    out.push_str(&shell_quote(&client_program.to_string_lossy()));
    for arg in client_argv {
        out.push(' ');
        out.push_str(&shell_quote(arg));
    }
    out.push('\n');
    out
}

fn build_windows_launcher(
    client_program: &std::path::Path,
    client_argv: &[String],
    env: &[LaunchEnv],
) -> String {
    let mut out = String::from("@echo off\r\ndel \"%~f0\" >nul 2>&1\r\n");
    for var in env {
        out.push_str("set \"");
        out.push_str(var.name());
        out.push('=');
        out.push_str(&escape_cmd_set_value(var.value().expose()));
        out.push_str("\"\r\n");
    }
    out.push('"');
    out.push_str(&client_program.to_string_lossy().replace('"', "\"\""));
    out.push('"');
    for arg in client_argv {
        out.push(' ');
        out.push('"');
        out.push_str(&arg.replace('"', "\"\""));
        out.push('"');
    }
    out.push_str("\r\n");
    out
}

fn launcher_script_path(name: &str) -> String {
    format!("/tmp/dblocal-open-client/{name}")
}

fn windows_launcher_script_path(name: &str) -> String {
    format!("%TEMP%\\dblocal-open-client\\{name}")
}

fn escape_applescript(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn escape_cmd_set_value(value: &str) -> String {
    // In `set "NAME=value"`, the main dangerous char is `"`. We also avoid
    // newlines.
    value.replace('\r', "").replace('\n', "").replace('"', "\"\"")
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
