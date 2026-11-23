//! Command handlers. Each handler is a library function over `&dyn DblService`
//! with injected writers, so tests can drive them without a real terminal.

pub mod autostart;
pub mod config;
pub mod create;
pub mod db;
pub mod delete;
pub mod list;
pub mod logs;
pub mod open;
pub mod password;
pub mod start;
pub mod status;
pub mod stop;
pub mod update;
pub mod uri;
pub mod user;

use std::io::Write;

use dblocal_core::service::DblService;

use crate::cli::Command;
use crate::output;

/// Reads the typed confirmation for `delete` when it is interactive.
///
/// Injected so a test can prove the refusal and the accept path without
/// touching stdin; production reads one line from the terminal.
pub type Confirm<'a> = &'a dyn Fn() -> Option<String>;

/// Context passed to every command handler.
pub struct Ctx<'a> {
    pub service: &'a dyn DblService,
    pub out: &'a mut dyn Write,
    pub err: &'a mut dyn Write,
    pub is_interactive: bool,
    pub stdout_is_tty: bool,
    pub stderr_is_tty: bool,
    /// Source of the typed instance name for an interactive delete.
    pub confirm: Confirm<'a>,
}

/// Dispatches a parsed command. Returns an exit code.
pub fn dispatch(ctx: &mut Ctx<'_>, command: &Command) -> u8 {
    match command {
        Command::List { json } => list::run(ctx, *json),
        Command::Status { name } => status::run(ctx, name),
        Command::Start { name, timeout } => start::run(ctx, name, *timeout),
        Command::Stop { name } => stop::run(ctx, name),
        Command::Uri { name, env } => uri::run(ctx, name, *env),
        Command::Create {
            engine,
            version,
            name,
            port,
            no_password,
        } => create::run(ctx, engine, version, name, *port, *no_password),
        Command::Delete { name, yes, purge } => delete::run(ctx, name, *yes, *purge),
        Command::Logs {
            name,
            follow,
            lines,
        } => logs::run(ctx, name, *follow, *lines),
        Command::Autostart { action, json } => {
            autostart::run_action(ctx.out, ctx.err, action, *json)
        }
        Command::AutostartRun { .. } => {
            // Handled by `cli::run` before the service is built; reaching here
            // means a caller wired the service path by mistake.
            output::stderr_line(
                ctx.err,
                &crate::messages::code_line(
                    dblocal_core::error::CODE_INVALID_INPUT,
                    "autostart-run tidak tersedia di mode ini",
                ),
            );
            crate::cli::EXIT_BAD_ARGS
        }
        Command::Db { command } => db::dispatch(ctx, command),
        Command::User { command } => user::dispatch(ctx, command),
        Command::Password { command } => password::dispatch(ctx, command),
        Command::Update { command } => update::dispatch(ctx, command),
        Command::Open {
            instance,
            client,
            dry_run,
        } => open::run(ctx, instance, client, *dry_run),
        Command::Config { command } => config::dispatch(ctx, command),
    }
}

/// Resolves an instance by name, returning its view or printing not-found and returning exit 3.
pub fn find_instance(
    ctx: &mut Ctx<'_>,
    name: &str,
) -> Result<dblocal_core::service::InstanceView, u8> {
    let views = match ctx.service.list_instances() {
        Ok(v) => v,
        Err(e) => return Err(output::report_error(ctx.err, &e)),
    };
    match views.into_iter().find(|v| v.name == name) {
        Some(v) => Ok(v),
        None => {
            output::stderr_line(
                ctx.err,
                &crate::messages::code_line(
                    dblocal_core::error::CODE_NOT_FOUND,
                    &format!("{}: {name}", crate::messages::ERR_INSTANCE_NOT_FOUND),
                ),
            );
            Err(crate::cli::EXIT_NOT_FOUND)
        }
    }
}

#[cfg(test)]
mod tests;
