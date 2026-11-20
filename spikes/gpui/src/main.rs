use std::io::{self, Write};
use std::process::ExitCode;

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_target(false)
        .init();

    if let Err(report) = gpui_spike::vulkan::preflight() {
        let _ = writeln!(io::stderr(), "{report}");
        return ExitCode::from(78);
    }

    gpui_spike::app::run();
    ExitCode::SUCCESS
}
