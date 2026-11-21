use std::time::Duration;

use dblocal_core::model::InstanceStatus;

use crate::cli::{EXIT_GENERAL, EXIT_OK};
use crate::commands::{Ctx, find_instance};
use crate::messages;
use crate::output;

pub fn run(ctx: &mut Ctx<'_>, name: &str) -> u8 {
    let view = match find_instance(ctx, name) {
        Ok(v) => v,
        Err(code) => return code,
    };
    if view.status == InstanceStatus::Stopped {
        output::stderr_line(ctx.err, "sudah berhenti");
        return EXIT_OK;
    }
    output::stderr_line(ctx.err, &format!("{} {name}...", messages::MSG_STOPPING));
    let mut rx = ctx.service.subscribe();
    let op = match ctx.service.stop_instance(&view.id) {
        Ok(op) => op,
        Err(e) => return output::report_error(ctx.err, &e),
    };
    let wait = output::StatusWait {
        op: &op,
        instance_id: &view.id,
        ok_statuses: &[InstanceStatus::Stopped],
        timeout: Duration::from_secs(crate::consts::STOP_TIMEOUT_SECS),
        show_progress: ctx.stderr_is_tty,
    };
    match output::wait_for_status_rx(&mut rx, ctx.service, wait, ctx.err) {
        Ok(_) => {
            output::stderr_line(ctx.err, messages::MSG_READY);
            EXIT_OK
        }
        Err(e) => {
            output::stderr_line(ctx.err, &format!("[{}] {}", e.code, e.message));
            EXIT_GENERAL
        }
    }
}
