use dblocal_core::model::{AuthMode, EngineKey, InstanceName, InstanceSpec, Port};
use dblocal_core::service::UriFormat;

use crate::cli::{EXIT_BAD_ARGS, EXIT_GENERAL, EXIT_OK};
use crate::commands::Ctx;
use crate::messages;
use crate::output;

pub fn run(
    ctx: &mut Ctx<'_>,
    engine: &str,
    version: &str,
    name: &str,
    port: Option<u16>,
    no_password: bool,
) -> u8 {
    let engine_key = match engine.parse::<EngineKey>() {
        Ok(e) => e,
        Err(_) => {
            output::stderr_line(
                ctx.err,
                &format!("[DBL-1001] {}: {engine}", messages::ERR_UNKNOWN_ENGINE),
            );
            return EXIT_BAD_ARGS;
        }
    };
    let inst_name = match InstanceName::new(name) {
        Ok(n) => n,
        Err(e) => return output::report_error(ctx.err, &e),
    };
    let port = match port {
        Some(p) => match Port::new(p) {
            Ok(p) => Some(p),
            Err(e) => return output::report_error(ctx.err, &e),
        },
        None => None,
    };
    let auth = if no_password {
        AuthMode::None
    } else {
        AuthMode::Password
    };
    let spec = match InstanceSpec::new(inst_name, engine_key, version.to_owned(), port, auth) {
        Ok(s) => s,
        Err(e) => return output::report_error(ctx.err, &e),
    };

    if let Err(e) = ctx.service.validate_spec(&spec) {
        return output::report_error(ctx.err, &e);
    }

    output::stderr_line(
        ctx.err,
        &format!(
            "{} {name} ({} {})...",
            messages::MSG_CREATING,
            engine_key.as_str(),
            version
        ),
    );

    let mut rx = ctx.service.subscribe();
    let op = match ctx.service.create_instance(spec) {
        Ok(op) => op,
        Err(e) => return output::report_error(ctx.err, &e),
    };

    let res = output::wait_operation_rx(&mut rx, &op, ctx.err, ctx.stderr_is_tty);
    match res {
        Ok(()) => {
            let view = match ctx.service.list_instances() {
                Ok(v) => v.into_iter().find(|v| v.name == name),
                Err(e) => return output::report_error(ctx.err, &e),
            };
            let Some(view) = view else {
                output::stderr_line(ctx.err, "[DBL-1002] instance missing after create");
                return crate::cli::EXIT_NOT_FOUND;
            };
            match ctx.service.connection_uri(&view.id, UriFormat::Uri) {
                Ok(uri) => {
                    output::stdout_line(ctx.out, uri.expose());
                    EXIT_OK
                }
                Err(e) => output::report_error(ctx.err, &e),
            }
        }
        Err(e) => {
            output::stderr_line(ctx.err, &format!("[{}] {}", e.code, e.message));
            if matches!(
                e.code.as_str(),
                "DBL-1001" | "DBL-1003" | "DBL-1005" | "DBL-6001"
            ) {
                EXIT_BAD_ARGS
            } else if e.code == "DBL-4001" {
                crate::cli::EXIT_ENGINE_FAILED
            } else {
                EXIT_GENERAL
            }
        }
    }
}
