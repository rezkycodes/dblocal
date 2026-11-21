use crate::cli::EXIT_OK;
use crate::commands::Ctx;
use crate::output;

pub fn run(ctx: &mut Ctx<'_>, json: bool) -> u8 {
    let instances = match ctx.service.list_instances() {
        Ok(v) => v,
        Err(e) => return output::report_error(ctx.err, &e),
    };
    if json {
        match output::format_list_json(&instances) {
            Ok(s) => {
                output::stdout_line(ctx.out, &s);
                EXIT_OK
            }
            Err(e) => {
                output::stderr_line(ctx.err, &format!("[DBL-3001] json: {e}"));
                crate::cli::EXIT_GENERAL
            }
        }
    } else {
        let table = output::format_list_table(&instances, ctx.stdout_is_tty);
        output::stdout_line(ctx.out, &table);
        EXIT_OK
    }
}
