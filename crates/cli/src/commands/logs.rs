use std::io::{self, Write};
use std::time::Duration;

use crate::cli::EXIT_OK;
use crate::commands::{find_instance, Ctx};
use crate::output;

pub fn run(ctx: &mut Ctx<'_>, name: &str, follow: bool, lines: usize) -> u8 {
    let view = match find_instance(ctx, name) {
        Ok(v) => v,
        Err(code) => return code,
    };

    match ctx.service.tail_logs(&view.id, lines) {
        Ok(chunk) => {
            for line in chunk {
                output::stdout_line(ctx.out, &line);
            }
        }
        Err(e) => return output::report_error(ctx.err, &e),
    }

    if !follow {
        return EXIT_OK;
    }

    // Follow by polling the service tail. Ctrl-C is handled by the OS; we exit
    // 0 when the process receives SIGINT (default handler terminates). For a
    // clean exit on Ctrl-C in Rust, we set a flag via ctrlc is not a dep; use
    // signal_hook is not in deps. Instead, loop and rely on SIGINT terminating
    // process with default behavior? The spec says "until Ctrl-C (exit 0 on SIGINT)".
    // We'll install a simple handler using libc-free approach? On unix, SIGINT
    // default kills; to exit 0 we need to catch it. Add `signal-hook` or
    // `ctrlc`? Neither is in deps. We can use a tokio signal if we have a runtime.
    // Minimal: use std::sync::atomic flag set by unsafe libc signal? No unsafe.
    // Alternative: spawn a thread that reads stdin? No. Better: use nix sigaction? nix is in workspace deps on unix.
    // Simpler: just loop and on broken pipe / signal the process exits; but SIGINT gives 130.
    // I'll implement a small SIGINT handler via tokio::signal in a current-thread runtime.
    follow_loop(ctx, &view)
}

#[cfg(unix)]
fn follow_loop(ctx: &mut Ctx<'_>, view: &dblocal_core::service::InstanceView) -> u8 {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    let stop = Arc::new(AtomicBool::new(false));
    let stop2 = stop.clone();
    let handle = std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build();
        if let Ok(rt) = rt {
            rt.block_on(async {
                if tokio::signal::ctrl_c().await.is_ok() {
                    stop2.store(true, Ordering::SeqCst);
                }
            });
        }
    });
    let mut last_seen = String::new();
    while !stop.load(Ordering::SeqCst) {
        std::thread::sleep(Duration::from_millis(250));
        if stop.load(Ordering::SeqCst) {
            break;
        }
        match ctx.service.tail_logs(&view.id, 200) {
            Ok(chunk) => {
                // naive: print lines that weren't in last snapshot tail - better: track offset by count
                // Simple approach: keep last N lines seen as a joined fingerprint.
                let joined = chunk.join("\n");
                if joined != last_seen {
                    // print only new suffix; compute common prefix lines
                    let old: Vec<&str> = if last_seen.is_empty() { vec![] } else { last_seen.split('\n').collect() };
                    let new_lines: Vec<&str> = joined.split('\n').collect();
                    // find overlap
                    let start = common_suffix_prefix(&old, &new_lines);
                    for line in &new_lines[start..] {
                        if !line.is_empty() || new_lines.len() > 1 {
                            output::stdout_line(ctx.out, line);
                        }
                    }
                    let _ = io::stdout().flush();
                    last_seen = joined;
                }
            }
            Err(_) => break,
        }
    }
    drop(handle);
    EXIT_OK
}

#[cfg(not(unix))]
fn follow_loop(ctx: &mut Ctx<'_>, view: &dblocal_core::service::InstanceView) -> u8 {
    let mut last_seen = String::new();
    loop {
        std::thread::sleep(Duration::from_millis(250));
        match ctx.service.tail_logs(&view.id, 200) {
            Ok(chunk) => {
                let joined = chunk.join("\n");
                if joined != last_seen {
                    let old: Vec<&str> = if last_seen.is_empty() { vec![] } else { last_seen.split('\n').collect() };
                    let new_lines: Vec<&str> = joined.split('\n').collect();
                    let start = common_suffix_prefix(&old, &new_lines);
                    for line in &new_lines[start..] {
                        output::stdout_line(ctx.out, line);
                    }
                    let _ = io::stdout().flush();
                    last_seen = joined;
                }
            }
            Err(_) => break,
        }
    }
    #[allow(unreachable_code)]
    EXIT_OK
}

fn common_suffix_prefix(old: &[&str], new: &[&str]) -> usize {
    // find largest k such that suffix of old of len k == prefix of new of len k
    let max_k = old.len().min(new.len());
    for k in (0..=max_k).rev() {
        if old[old.len() - k..] == new[..k] {
            return k;
        }
    }
    0
}
