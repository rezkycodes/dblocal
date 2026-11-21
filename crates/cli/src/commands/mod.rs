//! Command handlers. Each handler is a library function over `&dyn DblService`
//! with injected writers, so tests can drive them without a real terminal.

pub mod create;
pub mod delete;
pub mod list;
pub mod logs;
pub mod start;
pub mod status;
pub mod stop;
pub mod uri;

use std::io::Write;

use dblocal_core::service::DblService;

use crate::cli::Command;
use crate::output;

/// Context passed to every command handler.
pub struct Ctx<'a> {
    pub service: &'a dyn DblService,
    pub out: &'a mut dyn Write,
    pub err: &'a mut dyn Write,
    pub is_interactive: bool,
    pub stdout_is_tty: bool,
    pub stderr_is_tty: bool,
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
        Command::Logs { name, follow, lines } => logs::run(ctx, name, *follow, *lines),
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
                &format!("[DBL-1002] {}: {name}", crate::messages::ERR_INSTANCE_NOT_FOUND),
            );
            Err(crate::cli::EXIT_NOT_FOUND)
        }
    }
}
