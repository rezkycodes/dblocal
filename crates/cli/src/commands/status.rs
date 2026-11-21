use dblocal_core::model::InstanceStatus;

use crate::cli::{EXIT_GENERAL, EXIT_OK};
use crate::commands::{find_instance, Ctx};
use crate::output::{self, status_label};

pub fn run(ctx: &mut Ctx<'_>, name: &str) -> u8 {
    let view = match find_instance(ctx, name) {
        Ok(v) => v,
        Err(code) => return code,
    };
    output::stdout_line(ctx.out, status_label(view.status));
    if view.status == InstanceStatus::Running {
        EXIT_OK
    } else {
        EXIT_GENERAL
    }
}
