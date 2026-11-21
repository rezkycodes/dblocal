use std::time::Duration;

use dblocal_core::model::InstanceStatus;

use crate::cli::{EXIT_ENGINE_FAILED, EXIT_OK};
use crate::commands::{find_instance, Ctx};
use crate::messages;
use crate::output;

pub fn run(ctx: &mut Ctx<'_>, name: &str, timeout: u64) -> u8 {
    let view = match find_instance(ctx, name) {
        Ok(v) => v,
        Err(code) => return code,
    };
    if view.status == InstanceStatus::Running {
        output::stderr_line(ctx.err, messages::ERR_ALREADY_RUNNING);
        // still wait until healthy (it is) - return 0
        return EXIT_OK;
    }
    output::stderr_line(ctx.err, &format!("{} {name}...", messages::MSG_STARTING));
    let mut rx = ctx.service.subscribe();
    let op = match ctx.service.start_instance(&view.id) {
        Ok(op) => op,
        Err(e) => return output::report_error(ctx.err, &e),
    };
    let timeout = Duration::from_secs(timeout.max(1));
    match output::wait_for_status_rx(
        &mut rx,
        ctx.service,
        &op,
        &view.id,
        &[InstanceStatus::Running],
        timeout,
        ctx.err,
        ctx.stderr_is_tty,
    ) {
        Ok(_) => {
            output::stderr_line(ctx.err, messages::MSG_READY);
            EXIT_OK
        }
        Err(e) => {
            output::stderr_line(ctx.err, &format!("[{}] {}", e.code, e.message));
            if let Some(diag) = e.diagnosis {
                output::stderr_line(ctx.err, &format!("diagnosis: {}", diag.as_str()));
            }
            if e.code == "DBL-4001" { EXIT_ENGINE_FAILED } else if e.code == "DBL-1002" { crate::cli::EXIT_NOT_FOUND } else { crate::cli::EXIT_GENERAL }
        }
    }
}
