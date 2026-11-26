use dblocal_core::model::{AuthMode, EngineKey, InstanceName, InstanceSpec, InstanceStatus, Port};
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
    console: bool,
) -> u8 {
    let spec = match build_spec(ctx, engine, version, name, port, no_password, console) {
        Ok(spec) => spec,
        Err(code) => return code,
    };
    let engine_key = spec.engine();

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
    run_create(ctx, spec, name)
}

/// Starts the create operation and reports its outcome.
fn run_create(ctx: &mut Ctx<'_>, spec: InstanceSpec, name: &str) -> u8 {
    let mut rx = ctx.service.subscribe();
    let op = match ctx.service.create_instance(spec) {
        Ok(op) => op,
        Err(e) => return output::report_error(ctx.err, &e),
    };
    let res = output::wait_operation_rx(
        &mut rx,
        &op,
        ctx.service,
        output::Target::Name(name),
        output::Completion::StatusIn(&[InstanceStatus::Stopped]),
        ctx.err,
        ctx.stderr_is_tty,
    );
    match res {
        Ok(()) => print_created_uri(ctx, name),
        Err(e) => {
            output::stderr_line(ctx.err, &messages::code_line(&e.code, &e.message));
            exit_for_create_failure(&e.code)
        }
    }
}

/// Prints the finished instance's URI, or reports that the row is gone.
fn print_created_uri(ctx: &mut Ctx<'_>, name: &str) -> u8 {
    let view = match ctx.service.list_instances() {
        Ok(v) => v.into_iter().find(|v| v.name == name),
        Err(e) => return output::report_error(ctx.err, &e),
    };
    let Some(view) = view else {
        output::stderr_line(
            ctx.err,
            &messages::code_line(
                dblocal_core::error::CODE_NOT_FOUND,
                messages::ERR_CREATE_MISSING,
            ),
        );
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

/// `create`'s exit code for a failed operation (F-CLI-03 AC2).
fn exit_for_create_failure(code: &str) -> u8 {
    if matches!(code, "DBL-1001" | "DBL-1003" | "DBL-1005" | "DBL-6001") {
        EXIT_BAD_ARGS
    } else if code == "DBL-4001" {
        crate::cli::EXIT_ENGINE_FAILED
    } else {
        EXIT_GENERAL
    }
}

/// Parses and validates every boundary value into one `InstanceSpec`.
fn build_spec(
    ctx: &mut Ctx<'_>,
    engine: &str,
    version: &str,
    name: &str,
    port: Option<u16>,
    no_password: bool,
    console: bool,
) -> Result<InstanceSpec, u8> {
    let engine_key = match engine.parse::<EngineKey>() {
        Ok(e) => e,
        Err(_) => {
            output::stderr_line(
                ctx.err,
                &messages::code_line(
                    dblocal_core::error::CODE_INVALID_INPUT,
                    &format!("{}: {engine}", messages::ERR_UNKNOWN_ENGINE),
                ),
            );
            return Err(EXIT_BAD_ARGS);
        }
    };
    let inst_name = InstanceName::new(name).map_err(|e| output::report_error(ctx.err, &e))?;
    let port = match port {
        Some(value) => Some(Port::new(value).map_err(|e| output::report_error(ctx.err, &e))?),
        None => None,
    };
    let auth = if no_password {
        AuthMode::None
    } else {
        AuthMode::Password
    };
    let spec = InstanceSpec::new(inst_name, engine_key, version.to_owned(), port, auth)
        .map_err(|e| output::report_error(ctx.err, &e))?;
    Ok(spec.with_console_enabled(console))
}
