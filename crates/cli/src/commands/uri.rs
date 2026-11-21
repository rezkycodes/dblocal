use dblocal_core::service::UriFormat;

use crate::cli::EXIT_OK;
use crate::commands::{find_instance, Ctx};
use crate::output;

pub fn run(ctx: &mut Ctx<'_>, name: &str, env: bool) -> u8 {
    let view = match find_instance(ctx, name) {
        Ok(v) => v,
        Err(code) => return code,
    };
    let format = if env { UriFormat::DotEnv } else { UriFormat::Uri };
    match ctx.service.connection_uri(&view.id, format) {
        Ok(secret) => {
            // URI is the one command that may print a password; print only the value.
            output::stdout_line(ctx.out, secret.expose());
            EXIT_OK
        }
        Err(e) => output::report_error(ctx.err, &e),
    }
}
