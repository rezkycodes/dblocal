//! `dblocal user create` (`F-DBU-02`).
//!
//! The generated password is printed exactly once to stdout, like `uri`; nothing
//! else on stdout. The service stores it nowhere.

use dblocal_core::admin::PrivilegeLevel;

use crate::cli::{EXIT_BAD_ARGS, EXIT_OK, UserCommand};
use crate::commands::{Ctx, find_instance};
use crate::messages;
use crate::output;

pub fn dispatch(ctx: &mut Ctx<'_>, command: &UserCommand) -> u8 {
    match command {
        UserCommand::Create {
            instance,
            name,
            db,
            level,
        } => create(ctx, instance, name, db, level),
    }
}

fn create(ctx: &mut Ctx<'_>, instance: &str, name: &str, db: &str, level: &str) -> u8 {
    let level = match parse_level(level) {
        Some(level) => level,
        None => {
            output::stderr_line(
                ctx.err,
                &messages::code_line(
                    dblocal_core::error::CODE_INVALID_INPUT,
                    messages::ERR_BAD_LEVEL,
                ),
            );
            return EXIT_BAD_ARGS;
        }
    };
    let view = match find_instance(ctx, instance) {
        Ok(view) => view,
        Err(code) => return code,
    };
    match ctx.service.create_user(&view.id, name, db, level) {
        Ok(password) => {
            output::stderr_line(ctx.err, messages::MSG_USER_CREATED);
            output::stdout_line(ctx.out, password.expose());
            EXIT_OK
        }
        Err(err) => output::report_facing(ctx.err, &err),
    }
}

/// Maps `ro`/`rw` to a privilege level; anything else is a bad argument.
fn parse_level(level: &str) -> Option<PrivilegeLevel> {
    match level {
        "ro" => Some(PrivilegeLevel::ReadOnly),
        "rw" => Some(PrivilegeLevel::ReadWrite),
        _ => None,
    }
}
