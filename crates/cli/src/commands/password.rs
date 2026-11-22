//! `dblocal password reset` (`F-DBU-03`).
//!
//! After the reset the connect URI is re-rendered from the service and printed
//! once to stdout, exactly like `uri`, so the password the engine now holds is
//! the one in the printed URI (`F-DBU-03` AC1).

use dblocal_core::service::UriFormat;

use crate::cli::{EXIT_OK, PasswordCommand};
use crate::commands::{Ctx, find_instance};
use crate::messages;
use crate::output;

pub fn dispatch(ctx: &mut Ctx<'_>, command: &PasswordCommand) -> u8 {
    match command {
        PasswordCommand::Reset { instance } => reset(ctx, instance),
    }
}

fn reset(ctx: &mut Ctx<'_>, instance: &str) -> u8 {
    let view = match find_instance(ctx, instance) {
        Ok(view) => view,
        Err(code) => return code,
    };
    if let Err(err) = ctx.service.reset_superuser_password(&view.id) {
        return output::report_facing(ctx.err, &err);
    }
    // Re-read the URI so it carries the new password; the reset already updated
    // the stored credential.
    match ctx.service.connection_uri(&view.id, UriFormat::Uri) {
        Ok(uri) => {
            output::stderr_line(ctx.err, messages::MSG_PASSWORD_RESET);
            output::stdout_line(ctx.out, uri.expose());
            EXIT_OK
        }
        Err(err) => output::report_error(ctx.err, &err),
    }
}
