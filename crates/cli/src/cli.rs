//! Command-line surface and dispatch.

use std::ffi::OsString;
use std::io::{self, Write};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use dblocal_core::DblError;
use dblocal_core::paths::DblPaths;
use dblocal_core::service::{DblService, LocalService, ServiceOptions};

use crate::commands::{self, Ctx};
use crate::messages;
use crate::output;

/// Exit code for success, per `F-CLI`.
pub const EXIT_OK: u8 = 0;
/// Exit code for a general error, per `F-CLI`.
pub const EXIT_GENERAL: u8 = 1;
/// Exit code for bad arguments, per `F-CLI`.
pub const EXIT_BAD_ARGS: u8 = 2;
/// Exit code for an instance that does not exist, per `F-CLI`.
pub const EXIT_NOT_FOUND: u8 = 3;
/// Exit code for an engine that failed to start, per `F-CLI`.
pub const EXIT_ENGINE_FAILED: u8 = 4;

/// DBLocal command-line interface.
///
/// Help text is English, like the GUI; flags stay English too.
#[derive(Debug, Parser)]
#[command(
    name = "dblocal",
    version,
    about = "DBLocal — local database servers without containers",
    long_about = "DBLocal manages PostgreSQL, MySQL, MariaDB and Valkey servers, plus \n\
                  Redis (Linux only) and RustFS object storage, on your local machine. \n\
                  Without arguments the GUI opens; with a subcommand it runs in CLI mode.",
    after_help = "Without arguments, the DBLocal window opens."
)]
pub struct Cli {
    /// The subcommand to run. Without a subcommand the GUI opens.
    #[command(subcommand)]
    pub command: Option<Command>,
}

/// Every subcommand in PRD §8.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// List instances.
    List {
        /// Print JSON that can be piped.
        #[arg(long)]
        json: bool,
    },
    /// Show the status of one instance.
    Status {
        /// Instance name.
        name: String,
    },
    /// Start an instance and wait until it is healthy.
    Start {
        /// Instance name.
        name: String,
        /// Seconds to wait until the instance is healthy.
        #[arg(long, default_value_t = crate::consts::DEFAULT_START_TIMEOUT_SECS)]
        timeout: u64,
    },
    /// Stop an instance.
    Stop {
        /// Instance name.
        name: String,
    },
    /// Print the connection URI (or the .env block).
    Uri {
        /// Instance name.
        name: String,
        /// Print the `.env` block instead of only the URI.
        #[arg(long)]
        env: bool,
    },
    /// Create a new instance without prompting.
    Create {
        /// Engine: postgresql, mysql, mariadb, valkey, redis or rustfs.
        #[arg(long)]
        engine: String,
        /// Engine version tag, e.g. 18.7.0.
        #[arg(long)]
        version: String,
        /// Instance name.
        #[arg(long)]
        name: String,
        /// Port to use; empty means one is chosen automatically.
        #[arg(long)]
        port: Option<u16>,
        /// Create without a password.
        #[arg(long = "no-password")]
        no_password: bool,
        /// Enable the optional web console (RustFS; OFF by default).
        #[arg(long)]
        console: bool,
    },
    /// Delete an instance (the data is archived by default).
    Delete {
        /// Instance name.
        name: String,
        /// Skip the confirmation.
        #[arg(long)]
        yes: bool,
        /// Delete the data too instead of archiving it.
        #[arg(long)]
        purge: bool,
    },
    /// Show an instance log.
    Logs {
        /// Instance name.
        name: String,
        /// Follow new lines until Ctrl-C.
        #[arg(long, short)]
        follow: bool,
        /// Number of trailing lines to show.
        #[arg(long, default_value_t = crate::consts::DEFAULT_LOG_LINES)]
        lines: usize,
    },
    /// Manage the app autostart at login (status, enable, disable).
    Autostart {
        /// Action: status, enable or disable.
        action: String,
        /// Print JSON that can be piped.
        #[arg(long)]
        json: bool,
    },
    /// Login-item mode: open to the tray without a window and start the flagged instances.
    ///
    /// The login entry created by `dblocal autostart enable` passes this flag
    /// itself; it is not normally run by hand.
    #[command(name = "autostart-run")]
    AutostartRun {
        /// Keep running headless even when the tray is unavailable.
        #[arg(long)]
        headless: bool,
    },
    /// Manage databases on one instance (F-DBU-01).
    Db {
        #[command(subcommand)]
        command: DbCommand,
    },
    /// Manage database users on one instance (F-DBU-02).
    User {
        #[command(subcommand)]
        command: UserCommand,
    },
    /// Change the superuser password of one instance (F-DBU-03).
    Password {
        #[command(subcommand)]
        command: PasswordCommand,
    },
    /// Check for and install DBLocal updates (F-PKG-05).
    Update {
        #[command(subcommand)]
        command: UpdateCommand,
    },
    /// Open an instance in an external client (F-OPN).
    Open {
        /// Instance name.
        instance: String,
        /// Client to open: psql, mysql, mariadb, valkey-cli, redis-cli,
        /// tableplus, dbeaver or dbdelve.
        #[arg(long)]
        client: String,
        /// Print the launch plan without running the client.
        #[arg(long = "dry-run")]
        dry_run: bool,
    },
    /// Show, change and restore the engine configuration (F-CFG).
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    /// List installed engine runtimes and remove an unused one (F-RTM-04).
    Runtime {
        #[command(subcommand)]
        command: RuntimeCommand,
    },
    /// Install, switch and remove PHP, Node.js and Composer (v0.3.0).
    Toolchain {
        #[command(subcommand)]
        command: ToolchainCommand,
    },
}

/// `dblocal toolchain ...` subcommands.
///
/// A noun (`toolchain`, not bare `install`) because `dblocal list` already lists
/// instances; see `docs/toolchains.md`.
#[derive(Debug, Subcommand)]
pub enum ToolchainCommand {
    /// List available or installed toolchain versions.
    List {
        /// Restrict to one tool: php, node or composer.
        tool: Option<String>,
        /// Show only versions installed on this machine.
        #[arg(long)]
        installed: bool,
        /// Print JSON that can be piped.
        #[arg(long)]
        json: bool,
    },
    /// Install a version. Activates it only when the tool has no active version.
    Install {
        /// Tool: php, node or composer.
        tool: String,
        /// Version, series or `latest` (e.g. `php 8.4`, `node 24`).
        spec: String,
        /// PHP profile on Linux: common or bulk.
        #[arg(long)]
        profile: Option<String>,
        /// Install without activating it, even when nothing is active.
        #[arg(long = "no-activate")]
        no_activate: bool,
        /// Skip the confirmation prompt.
        #[arg(long)]
        yes: bool,
    },
    /// Switch the active version of a tool to an installed one.
    Use {
        /// Tool: php, node or composer.
        tool: String,
        /// Installed version, series or `latest`.
        spec: String,
    },
    /// Remove an installed version.
    Remove {
        /// Tool: php, node or composer.
        tool: String,
        /// Installed version, series or `latest`.
        spec: String,
        /// Skip the confirmation prompt.
        #[arg(long)]
        yes: bool,
    },
    /// Print the path of the active executable of a tool.
    Which {
        /// Tool: php, node or composer.
        tool: String,
    },
    /// Inspect or change DBLocal's PATH entry (consent required to change it).
    Path {
        /// Print the lines that would be added, without writing.
        #[arg(long)]
        print: bool,
        /// Add the managed PATH block.
        #[arg(long)]
        apply: bool,
        /// Remove the managed PATH block, restoring the files.
        #[arg(long)]
        remove: bool,
    },
    /// Check PATH, shims and installs for problems.
    Doctor {
        /// Print JSON that can be piped.
        #[arg(long)]
        json: bool,
    },
    /// Read or change the managed PHP settings.
    PhpSettings {
        /// Action: get, set <key> <value>, reset, or open.
        action: String,
        /// Setting key for `set`.
        key: Option<String>,
        /// Setting value for `set`.
        value: Option<String>,
        /// Print JSON that can be piped (for `get`).
        #[arg(long)]
        json: bool,
    },
}

/// `dblocal runtime ...` subcommands.
#[derive(Debug, Subcommand)]
pub enum RuntimeCommand {
    /// List installed runtimes with their size and the instances using them.
    List {
        /// Print JSON that can be piped.
        #[arg(long)]
        json: bool,
    },
    /// Remove an unused runtime and the space it takes.
    Remove {
        /// Engine: postgresql, mysql, mariadb, valkey, redis or rustfs.
        engine: String,
        /// Engine version tag, e.g. 18.7.0.
        version: String,
        /// Skip the confirmation.
        #[arg(long)]
        yes: bool,
    },
}

/// `dblocal config ...` subcommands.
#[derive(Debug, Subcommand)]
pub enum ConfigCommand {
    /// Show the instance settings with their current and default values.
    Show {
        /// Instance name.
        name: String,
        /// Print JSON that can be piped.
        #[arg(long)]
        json: bool,
    },
    /// Change one or more settings (`key=value`).
    Set {
        /// Instance name.
        name: String,
        /// A `key=value` pair, e.g. `max_connections=200`.
        changes: Vec<String>,
    },
    /// Restore the configuration from the newest backup.
    Restore {
        /// Instance name.
        name: String,
    },
    /// Print the path of the raw configuration file.
    Path {
        /// Instance name.
        name: String,
    },
    /// Open the raw configuration file in the OS default editor (F-CFG-02).
    Open {
        /// Instance name.
        name: String,
    },
}

/// `dblocal db ...` subcommands.
#[derive(Debug, Subcommand)]
pub enum DbCommand {
    /// List databases.
    List {
        /// Instance name.
        instance: String,
        /// Print JSON that can be piped.
        #[arg(long)]
        json: bool,
    },
    /// Create a database.
    Create {
        /// Instance name.
        instance: String,
        /// Database name.
        name: String,
    },
    /// Drop a database. Requires the name to be confirmed again (F-DBU-01 AC3).
    Drop {
        /// Instance name.
        instance: String,
        /// Database name.
        name: String,
        /// Skip confirmation.
        #[arg(long)]
        yes: bool,
        /// Retype the database name here; must match `name`.
        #[arg(long)]
        confirm: Option<String>,
    },
}

/// `dblocal user ...` subcommands.
#[derive(Debug, Subcommand)]
pub enum UserCommand {
    /// Create a user with a given privilege level.
    Create {
        /// Instance name.
        instance: String,
        /// User name.
        name: String,
        /// Database the GRANT applies to.
        #[arg(long)]
        db: String,
        /// Privilege level: `ro` (read-only) or `rw` (read-write).
        #[arg(long)]
        level: String,
    },
}

/// `dblocal password ...` subcommands.
#[derive(Debug, Subcommand)]
pub enum PasswordCommand {
    /// Change the superuser password; the new URI is printed once to stdout.
    Reset {
        /// Instance name.
        instance: String,
    },
}

/// `dblocal update ...` subcommands.
#[derive(Debug, Subcommand)]
pub enum UpdateCommand {
    /// Check whether a newer version exists.
    Check {
        /// Print JSON that can be piped.
        #[arg(long)]
        json: bool,
    },
    /// Download, verify and install an update.
    Apply {
        /// Skip confirmation.
        #[arg(long)]
        yes: bool,
    },
}

/// The hidden flag the login item passes to the app.
///
/// `--autostart` is the canonical token in `crates/core/src/autostart/consts.rs`;
/// the parser also exposes a long subcommand so the mode is discoverable in
/// `--help`. The launched-by-login-item check reads the raw argv, so both the
/// bare flag and `autostart-run` work.
pub const AUTOSTART_FLAG: &str = dblocal_core::autostart::AUTOSTART_FLAG;

/// Entry point used by `main`; returns the process exit code.
pub fn run(args: Vec<OsString>) -> ExitCode {
    if args
        .first()
        .is_some_and(|a| a == dblocal_core::consts::APPLY_UPDATE_FLAG)
    {
        return run_apply_update_helper(&args[1..]);
    }
    let args = normalize_autostart_flag(args);
    let cli = match Cli::try_parse_from(std::iter::once(OsString::from("dblocal")).chain(args)) {
        Ok(cli) => cli,
        Err(err) => return clap_exit(err),
    };

    match cli.command {
        None => match dblocal_gui::run() {
            Ok(()) => ExitCode::from(EXIT_OK),
            Err(err) => report(err),
        },
        Some(Command::AutostartRun { headless }) => run_autostart_mode(headless),
        // `autostart` is a pure login-item operation: it needs no state database,
        // so it is dispatched before the service is built (a first run on a
        // login item must not fail because the store could not be opened).
        Some(Command::Autostart { action, json }) => run_autostart_command(&action, json),
        // Toolchains are their own small file tree, not instances, so they are
        // dispatched without the state database too.
        Some(Command::Toolchain { command }) => run_toolchain_command(&command),
        Some(command) => run_command(command),
    }
}

fn run_apply_update_helper(args: &[OsString]) -> ExitCode {
    let parsed = match parse_windows_helper_args(args) {
        Ok(parsed) => parsed,
        Err(msg) => {
            eprintln!("{msg}");
            return ExitCode::from(EXIT_BAD_ARGS);
        }
    };
    match dblocal_core::update::run_windows_apply_helper(&parsed) {
        Ok(()) => ExitCode::from(EXIT_OK),
        Err(err) => {
            eprintln!("{}", err.diagnostic());
            ExitCode::from(EXIT_GENERAL)
        }
    }
}

fn parse_windows_helper_args(
    args: &[OsString],
) -> std::result::Result<dblocal_core::update::WindowsHelperArgs, String> {
    let mut pid: Option<u32> = None;
    let mut msi: Option<std::path::PathBuf> = None;
    let mut staged: Option<std::path::PathBuf> = None;
    let mut target: Option<std::path::PathBuf> = None;
    let mut version: Option<String> = None;
    let mut error_file: Option<std::path::PathBuf> = None;

    let mut i = 0;
    while i < args.len() {
        let key = args[i].to_string_lossy();
        let next = args
            .get(i + 1)
            .ok_or_else(|| format!("missing value for {key}"))?;
        match key.as_ref() {
            "--pid" => {
                pid = Some(
                    next.to_string_lossy()
                        .parse::<u32>()
                        .map_err(|e| format!("invalid --pid: {e}"))?,
                );
            }
            "--msi" => msi = Some(std::path::PathBuf::from(next)),
            "--staged" => staged = Some(std::path::PathBuf::from(next)),
            "--target" => target = Some(std::path::PathBuf::from(next)),
            "--version" => {
                version = Some(next.to_string_lossy().into_owned());
            }
            "--error-file" => error_file = Some(std::path::PathBuf::from(next)),
            other => return Err(format!("unknown --apply-update argument: {other}")),
        }
        i += 2;
    }

    let pid = pid.ok_or_else(|| "missing --pid".to_owned())?;
    let target = target.ok_or_else(|| "missing --target".to_owned())?;
    if msi.is_none() && staged.is_none() {
        return Err("missing --msi or --staged".to_owned());
    }
    Ok(dblocal_core::update::WindowsHelperArgs {
        pid,
        msi,
        staged,
        target,
        version,
        error_file,
    })
}

fn run_autostart_command(action: &str, json: bool) -> ExitCode {
    let mut out = io::stdout().lock();
    let mut err = io::stderr().lock();
    ExitCode::from(commands::autostart::run_action(
        &mut out, &mut err, action, json,
    ))
}

fn normalize_autostart_flag(args: Vec<OsString>) -> Vec<OsString> {
    // The login item passes `--autostart`. Clap does not know it as a flag, so
    // map it to the hidden subcommand before parsing. A user who explicitly
    // wrote `autostart-run` keeps it.
    if args.iter().any(|a| a == AUTOSTART_FLAG) {
        let mut out = Vec::with_capacity(args.len());
        for arg in args {
            if arg == AUTOSTART_FLAG {
                out.push(OsString::from("autostart-run"));
            } else {
                out.push(arg);
            }
        }
        out
    } else {
        args
    }
}

fn run_autostart_mode(headless: bool) -> ExitCode {
    let code = commands::autostart::run_login_mode(headless);
    ExitCode::from(code)
}

/// Dispatches a `toolchain` command. It needs no state database, so it does not
/// build the service.
fn run_toolchain_command(command: &ToolchainCommand) -> ExitCode {
    ExitCode::from(commands::toolchain::run(command))
}

fn run_command(command: Command) -> ExitCode {
    let mut out = io::stdout().lock();
    let mut err = io::stderr().lock();
    let code = run_with_service(
        &command,
        &mut out,
        &mut err,
        output::is_interactive_stdin(),
        output::is_tty_stdout(),
        output::is_tty_stderr(),
    );
    ExitCode::from(code)
}

/// Builds the real service and dispatches a command. Testable via `run_with`.
fn run_with_service(
    command: &Command,
    out: &mut dyn Write,
    err: &mut dyn Write,
    is_interactive: bool,
    stdout_is_tty: bool,
    stderr_is_tty: bool,
) -> u8 {
    let paths = match DblPaths::from_env() {
        Ok(p) => p,
        Err(e) => {
            output::stderr_line(err, &messages::service_open_error(&e.diagnostic()));
            return EXIT_GENERAL;
        }
    };
    let service = match LocalService::new(paths, ServiceOptions::default()) {
        Ok(s) => s,
        Err(e) => {
            output::stderr_line(err, &messages::service_open_error(&e.diagnostic()));
            return EXIT_GENERAL;
        }
    };
    run_with(
        &service,
        command,
        out,
        err,
        is_interactive,
        stdout_is_tty,
        stderr_is_tty,
    )
}

/// Dispatches a command against an injected service. This is the library entry
/// used by tests: it takes `&dyn DblService`, injected writers, and an
/// `is_interactive` flag.
pub fn run_with(
    service: &dyn DblService,
    command: &Command,
    out: &mut dyn Write,
    err: &mut dyn Write,
    is_interactive: bool,
    stdout_is_tty: bool,
    stderr_is_tty: bool,
) -> u8 {
    run_with_confirm(
        service,
        command,
        out,
        err,
        is_interactive,
        stdout_is_tty,
        stderr_is_tty,
        &read_confirmation,
    )
}

/// Reads one line from stdin, or `None` when stdin is not readable.
fn read_confirmation() -> Option<String> {
    use std::io::BufRead as _;
    let mut line = String::new();
    match std::io::stdin().lock().read_line(&mut line) {
        Ok(_) if !line.is_empty() => Some(line),
        _ => None,
    }
}

/// The injectable form: `confirm` supplies the typed delete confirmation.
#[allow(clippy::too_many_arguments)]
pub fn run_with_confirm(
    service: &dyn DblService,
    command: &Command,
    out: &mut dyn Write,
    err: &mut dyn Write,
    is_interactive: bool,
    stdout_is_tty: bool,
    stderr_is_tty: bool,
    confirm: commands::Confirm<'_>,
) -> u8 {
    let mut ctx = Ctx {
        service,
        out,
        err,
        is_interactive,
        stdout_is_tty,
        stderr_is_tty,
        confirm,
    };
    commands::dispatch(&mut ctx, command)
}

fn clap_exit(err: clap::Error) -> ExitCode {
    // `--help` and `--version` are successful outcomes, not failures.
    let code = if err.use_stderr() {
        EXIT_BAD_ARGS
    } else {
        EXIT_OK
    };
    let _ = err.print();
    ExitCode::from(code)
}

fn report(err: DblError) -> ExitCode {
    eprintln!("{}", err.diagnostic());
    ExitCode::from(output::code_for(&err))
}

/// Maps an error to the exit code contract in `F-CLI`.
#[cfg(test)]
pub fn code_for(err: &DblError) -> u8 {
    output::code_for(err)
}

#[cfg(test)]
pub fn parse(args: &[&str]) -> dblocal_core::Result<Cli> {
    Cli::try_parse_from(std::iter::once("dblocal").chain(args.iter().copied())).map_err(|err| {
        DblError::InvalidInput {
            field: "arguments",
            reason: err.to_string(),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_prd_command_parses() {
        let cases: &[&[&str]] = &[
            &["list"],
            &["list", "--json"],
            &["status", "app"],
            &["start", "app"],
            &["start", "app", "--timeout", "30"],
            &["stop", "app"],
            &["uri", "app"],
            &["uri", "app", "--env"],
            &[
                "create",
                "--engine",
                "postgresql",
                "--version",
                "18.7",
                "--name",
                "app",
                "--port",
                "5432",
                "--no-password",
            ],
            &[
                "create",
                "--engine",
                "rustfs",
                "--version",
                "1.0.7",
                "--name",
                "s3-dev",
                "--console",
            ],
            &["delete", "app", "--yes", "--purge"],
            &["logs", "app", "--follow"],
            &["logs", "app", "--lines", "50"],
            &["autostart", "status"],
            &["autostart", "enable", "--json"],
            &["autostart", "disable"],
            &["autostart-run", "--headless"],
            &["db", "list", "app"],
            &["db", "list", "app", "--json"],
            &["db", "create", "app", "app_db"],
            &["db", "drop", "app", "app_db", "--yes"],
            &["db", "drop", "app", "app_db", "--confirm", "app_db"],
            &[
                "user",
                "create",
                "app",
                "report_user",
                "--db",
                "app_db",
                "--level",
                "ro",
            ],
            &["password", "reset", "app"],
            &["update", "check"],
            &["update", "check", "--json"],
            &["update", "apply", "--yes"],
            &["open", "app", "--client", "tableplus"],
            &["open", "app", "--client", "psql", "--dry-run"],
            &["runtime", "list"],
            &["runtime", "list", "--json"],
            &["runtime", "remove", "postgresql", "17.4", "--yes"],
        ];
        for args in cases {
            parse(args).unwrap_or_else(|err| panic!("{args:?}: {err}"));
        }
    }

    #[test]
    fn no_arguments_means_gui() {
        let cli = parse(&[]).expect("parses");
        assert!(cli.command.is_none());
    }

    #[test]
    fn unknown_subcommand_is_rejected() {
        assert!(parse(&["frobnicate"]).is_err());
        assert!(parse(&["start"]).is_err(), "missing required name");
        assert!(parse(&["create", "--engine", "postgresql"]).is_err());
    }

    #[test]
    fn unknown_subcommand_exits_two() {
        let code = run(vec![OsString::from("frobnicate")]);
        assert_eq!(code, ExitCode::from(EXIT_BAD_ARGS));
    }

    #[test]
    fn help_succeeds() {
        let code = run(vec![OsString::from("--help")]);
        assert_eq!(code, ExitCode::from(EXIT_OK));
    }

    #[test]
    fn the_login_item_flag_normalizes_to_the_autostart_mode() {
        // The login item passes the raw `--autostart` token; normalization turns
        // it into the hidden subcommand before parsing.
        let normalized = normalize_autostart_flag(vec![OsString::from(AUTOSTART_FLAG)]);
        let cli = Cli::try_parse_from(std::iter::once(OsString::from("dblocal")).chain(normalized))
            .expect("parses");
        assert!(matches!(cli.command, Some(Command::AutostartRun { .. })));
        // Without the flag, nothing changes.
        let plain = normalize_autostart_flag(vec![OsString::from("list")]);
        assert_eq!(plain, vec![OsString::from("list")]);
    }

    #[test]
    fn autostart_actions_parse_with_an_optional_json_flag() {
        match parse(&["autostart", "status"]).expect("parses").command {
            Some(Command::Autostart { action, json }) => {
                assert_eq!(action, "status");
                assert!(!json);
            }
            other => panic!("unexpected {other:?}"),
        }
        match parse(&["autostart", "enable", "--json"])
            .expect("parses")
            .command
        {
            Some(Command::Autostart { action, json }) => {
                assert_eq!(action, "enable");
                assert!(json);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn exit_codes_follow_the_contract() {
        assert_eq!(
            code_for(&DblError::NotFound {
                kind: "instance",
                name: "x".into()
            }),
            EXIT_NOT_FOUND
        );
        assert_eq!(
            code_for(&DblError::Engine {
                reason: "boom".into()
            }),
            EXIT_ENGINE_FAILED
        );
        assert_eq!(
            code_for(&DblError::InvalidInput {
                field: "name",
                reason: "bad".into()
            }),
            EXIT_BAD_ARGS
        );
        assert_eq!(
            code_for(&DblError::InvalidTransition {
                from: "stopped",
                to: "running"
            }),
            EXIT_BAD_ARGS
        );
        assert_eq!(
            code_for(&DblError::NotImplemented("x".into())),
            EXIT_GENERAL
        );
        assert_eq!(
            code_for(&DblError::Storage { reason: "x".into() }),
            EXIT_GENERAL
        );
    }
}
