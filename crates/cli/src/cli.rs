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
    /// Mengelola database pada satu instance (F-DBU-01).
    Db {
        #[command(subcommand)]
        command: DbCommand,
    },
    /// Mengelola user database pada satu instance (F-DBU-02).
    User {
        #[command(subcommand)]
        command: UserCommand,
    },
    /// Mengganti password superuser satu instance (F-DBU-03).
    Password {
        #[command(subcommand)]
        command: PasswordCommand,
    },
    /// Memeriksa dan memasang pembaruan DBLocal (F-PKG-05).
    Update {
        #[command(subcommand)]
        command: UpdateCommand,
    },
    /// Membuka instance di client eksternal (F-OPN).
    Open {
        /// Nama instance.
        instance: String,
        /// Client yang dibuka: psql, mysql, mariadb, valkey-cli, redis-cli,
        /// tableplus, dbeaver, atau dbdelve.
        #[arg(long)]
        client: String,
        /// Cetak rencana peluncuran tanpa menjalankan client.
        #[arg(long = "dry-run")]
        dry_run: bool,
    },
}

/// `dblocal db ...` subcommands.
#[derive(Debug, Subcommand)]
pub enum DbCommand {
    /// Menampilkan daftar database.
    List {
        /// Nama instance.
        instance: String,
        /// Cetak JSON yang bisa dipipe.
        #[arg(long)]
        json: bool,
    },
    /// Membuat database baru.
    Create {
        /// Nama instance.
        instance: String,
        /// Nama database.
        name: String,
    },
    /// Menghapus database. Wajib konfirmasi ulang nama (F-DBU-01 AC3).
    Drop {
        /// Nama instance.
        instance: String,
        /// Nama database.
        name: String,
        /// Lewati konfirmasi.
        #[arg(long)]
        yes: bool,
        /// Ketik ulang nama database di sini; harus sama dengan `name`.
        #[arg(long)]
        confirm: Option<String>,
    },
}

/// `dblocal user ...` subcommands.
#[derive(Debug, Subcommand)]
pub enum UserCommand {
    /// Membuat user dengan tingkat hak tertentu.
    Create {
        /// Nama instance.
        instance: String,
        /// Nama user.
        name: String,
        /// Database tujuan GRANT.
        #[arg(long)]
        db: String,
        /// Tingkat hak: `ro` (read-only) atau `rw` (read-write).
        #[arg(long)]
        level: String,
    },
}

/// `dblocal password ...` subcommands.
#[derive(Debug, Subcommand)]
pub enum PasswordCommand {
    /// Mengganti password superuser; URI baru dicetak sekali ke stdout.
    Reset {
        /// Nama instance.
        instance: String,
    },
}

/// `dblocal update ...` subcommands.
#[derive(Debug, Subcommand)]
pub enum UpdateCommand {
    /// Memeriksa apakah ada versi lebih baru.
    Check {
        /// Cetak JSON yang bisa dipipe.
        #[arg(long)]
        json: bool,
    },
    /// Mengunduh, memverifikasi, dan memasang pembaruan.
    Apply {
        /// Lewati konfirmasi.
        #[arg(long)]
        yes: bool,
    },
}

/// Entry point used by `main`; returns the process exit code.
pub fn run(args: Vec<OsString>) -> ExitCode {
    let cli = match Cli::try_parse_from(std::iter::once(OsString::from("dblocal")).chain(args)) {
        Ok(cli) => cli,
        Err(err) => return clap_exit(err),
    };

    match cli.command {
        None => match dblocal_gui::run() {
            Ok(()) => ExitCode::from(EXIT_OK),
            Err(err) => report(err),
        },
        Some(command) => run_command(command),
    }
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
