//! `dblocal update check|apply` (`F-PKG-05`).
//!
//! `check` is the only place a check runs, and only because the user asked; there
//! is no timer and no startup check. `apply` refuses a package-manager install
//! and points at the manager (`F-PKG-05` AC2).

use crate::cli::{EXIT_BAD_ARGS, EXIT_OK, UpdateCommand};
use crate::commands::Ctx;
use crate::messages;
use crate::output;

pub fn dispatch(ctx: &mut Ctx<'_>, command: &UpdateCommand) -> u8 {
    match command {
        UpdateCommand::Check { json } => check(ctx, *json),
        UpdateCommand::Apply { yes } => apply(ctx, *yes),
    }
}

fn check(ctx: &mut Ctx<'_>, json: bool) -> u8 {
    let status = match ctx.service.check_update() {
        Ok(status) => status,
        Err(err) => return output::report_facing(ctx.err, &err),
    };
    if json {
        output::stdout_line(ctx.out, &output::format_update_json(&status));
    } else if let Some(available) = &status.available {
        output::stdout_line(
            ctx.out,
            &format!("{} {}", messages::MSG_UPDATE_AVAILABLE, available.version),
        );
    } else {
        output::stdout_line(ctx.out, messages::MSG_UPDATE_UP_TO_DATE);
    }
    if let Some(hint) = &status.package_manager_hint {
        output::stderr_line(ctx.err, hint);
    }
    EXIT_OK
}

fn apply(ctx: &mut Ctx<'_>, yes: bool) -> u8 {
    if !yes && !ctx.is_interactive {
        output::stderr_line(
            ctx.err,
            &messages::code_line(
                dblocal_core::error::CODE_INVALID_INPUT,
                messages::ERR_UPDATE_NEEDS_YES,
            ),
        );
        return EXIT_BAD_ARGS;
    }
    match ctx.service.apply_update() {
        Ok(()) => {
            output::stderr_line(ctx.err, messages::MSG_UPDATE_APPLIED);
            EXIT_OK
        }
        Err(err) => {
            // A package-manager refusal is a hard "use your manager" answer, not
            // a transient failure; the code still drives the exit code and the
            // hint line is printed for context.
            let code = output::report_facing(ctx.err, &err);
            if err.code == dblocal_core::update::CODE_UPDATE_UNSUPPORTED_KIND {
                output::stderr_line(ctx.err, messages::MSG_UPDATE_PACKAGE_MANAGER);
            }
            code
        }
    }
}
