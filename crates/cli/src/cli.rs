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
/// Help text is Indonesian (matching the GUI) while flags stay English.
#[derive(Debug, Parser)]
#[command(
    name = "dblocal",
    version,
    about = "DBLocal — server database lokal tanpa container",
    long_about = "DBLocal mengelola server PostgreSQL, MySQL, MariaDB, dan Valkey \n\
                  di mesin lokal. Tanpa argumen, aplikasi GUI terbuka; dengan \n\
                  subperintah, mode CLI yang berjalan.",
    after_help = "Tanpa argumen, jendela DBLocal terbuka."
)]
pub struct Cli {
    /// Subperintah yang dijalankan. Tanpa subperintah, GUI terbuka.
    #[command(subcommand)]
    pub command: Option<Command>,
}

/// Setiap subperintah pada PRD §8.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Menampilkan daftar instance.
    List {
        /// Cetak JSON yang bisa dipipe.
        #[arg(long)]
        json: bool,
    },
    /// Menampilkan status satu instance.
    Status {
        /// Nama instance.
        name: String,
    },
    /// Menjalankan instance dan menunggu sampai sehat.
    Start {
        /// Nama instance.
        name: String,
        /// Detik menunggu sampai instance sehat.
        #[arg(long, default_value_t = crate::consts::DEFAULT_START_TIMEOUT_SECS)]
        timeout: u64,
    },
    /// Menghentikan instance.
    Stop {
        /// Nama instance.
        name: String,
    },
    /// Mencetak connection URI (atau blok .env).
    Uri {
        /// Nama instance.
        name: String,
        /// Cetak blok `.env` alih-alih URI saja.
        #[arg(long)]
        env: bool,
    },
    /// Membuat instance baru tanpa interaksi.
    Create {
        /// Engine: postgresql, mysql, mariadb, atau valkey.
        #[arg(long)]
        engine: String,
        /// Tag versi engine, mis. 18.7.0.
        #[arg(long)]
        version: String,
        /// Nama instance.
        #[arg(long)]
        name: String,
        /// Port yang dipakai; kosong berarti dipilih otomatis.
        #[arg(long)]
        port: Option<u16>,
        /// Buat tanpa password.
        #[arg(long = "no-password")]
        no_password: bool,
    },
    /// Menghapus instance (default mengarsipkan data).
    Delete {
        /// Nama instance.
        name: String,
        /// Lewati konfirmasi.
        #[arg(long)]
        yes: bool,
        /// Hapus data juga, bukan diarsipkan.
        #[arg(long)]
        purge: bool,
    },
    /// Menampilkan log instance.
    Logs {
        /// Nama instance.
        name: String,
        /// Ikuti baris baru sampai Ctrl-C.
        #[arg(long, short)]
        follow: bool,
        /// Jumlah baris terakhir yang ditampilkan.
        #[arg(long, default_value_t = crate::consts::DEFAULT_LOG_LINES)]
        lines: usize,
    },
    /// Mengatur autostart aplikasi saat login (status, enable, disable).
    Autostart {
        /// Aksi: status, enable, atau disable.
        action: String,
        /// Cetak JSON yang bisa dipipe.
        #[arg(long)]
        json: bool,
    },
    /// Mode login item: buka ke tray tanpa jendela dan jalankan instance bertanda.
    ///
    /// Flag ini dipasang sendiri oleh entri login yang dibuat `dblocal autostart
    /// enable`; biasanya tidak dipanggil manual.
    #[command(name = "autostart-run")]
    AutostartRun {
        /// Tetap jalankan headless walau tray tidak tersedia.
        #[arg(long)]
        headless: bool,
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
        Some(command) => run_command(command),
    }
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
            &["delete", "app", "--yes", "--purge"],
            &["logs", "app", "--follow"],
            &["logs", "app", "--lines", "50"],
            &["autostart", "status"],
            &["autostart", "enable", "--json"],
            &["autostart", "disable"],
            &["autostart-run", "--headless"],
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
