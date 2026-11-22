//! `dblocal open <instance> --client <kind>` (`F-OPN`).
//!
//! The plan is printed on `--dry-run` and never carries the password (that only
//! goes into a private launcher script). Without `--dry-run`, the client is
//! launched through the service.

use dblocal_core::clients::ClientKind;

use crate::cli::{EXIT_BAD_ARGS, EXIT_OK};
use crate::commands::{Ctx, find_instance};
use crate::messages;
use crate::output;

pub fn run(ctx: &mut Ctx<'_>, instance: &str, client: &str, dry_run: bool) -> u8 {
    let kind = match parse_client(client) {
        Some(kind) => kind,
        None => {
            output::stderr_line(
                ctx.err,
                &messages::code_line(
                    dblocal_core::error::CODE_INVALID_INPUT,
                    &format!("{}: {client}", messages::ERR_UNKNOWN_CLIENT),
                ),
            );
            return EXIT_BAD_ARGS;
        }
    };
    let view = match find_instance(ctx, instance) {
        Ok(view) => view,
        Err(code) => return code,
    };
    match ctx.service.open_in_client(&view.id, kind, dry_run) {
        Ok(plan) => {
            if dry_run {
                output::stderr_line(ctx.err, messages::MSG_CLIENT_DRY_RUN);
                output::stdout_line(ctx.out, &output::format_launch_plan(&plan));
            } else {
                output::stderr_line(ctx.err, messages::MSG_CLIENT_OPENED);
            }
            EXIT_OK
        }
        Err(err) => output::report_facing(ctx.err, &err),
    }
}

/// Maps a `--client` word to a [`ClientKind`].
fn parse_client(value: &str) -> Option<ClientKind> {
    let value = value.trim();
    [
        ClientKind::DbDelve,
        ClientKind::TablePlus,
        ClientKind::DBeaver,
        ClientKind::Psql,
        ClientKind::Mysql,
        ClientKind::Mariadb,
        ClientKind::ValkeyCli,
        ClientKind::RedisCli,
    ]
    .into_iter()
    .find(|kind| kind.as_str() == value)
}
