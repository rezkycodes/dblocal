//! The per-case render hang guard and its tests.
//!
//! A deadlocked render held the GPU mutex for 54 minutes in CI; an abort that
//! names the case is the only diagnostic a wedged process can emit. The guard is
//! armed *after* the render mutex is acquired, so a case queued behind others is
//! never blamed for time it spent waiting, and it stays armed through the draw.

use std::sync::Arc;
use std::time::Duration;

/// How long one case may run before the guard aborts the process.
///
/// Under heavy shared-host load a healthy render can take well over two minutes;
/// ten minutes still catches a real hang without killing slow-but-progressing
/// cases.
pub(crate) const RENDER_WATCHDOG_SECS: u64 = 600;

/// Aborts the process when a render case overruns its limit.
pub(crate) struct RenderWatchdog {
    done: Arc<std::sync::atomic::AtomicBool>,
}

impl RenderWatchdog {
    /// Arms a timer that calls `on_fire` when `limit` elapses before `Drop`.
    ///
    /// The callback is injectable so the ordering tests can observe a firing
    /// instead of aborting the whole test binary. Production passes `abort`.
    pub(crate) fn arm_with(
        name: &str,
        limit: Duration,
        on_fire: impl Fn() + Send + 'static,
    ) -> Self {
        let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = Arc::clone(&done);
        let label = name.to_owned();
        std::thread::spawn(move || {
            std::thread::sleep(limit);
            if !flag.load(std::sync::atomic::Ordering::SeqCst) {
                let msg = format!(
                    "RENDER HANG: {label} did not finish within {}s; aborting",
                    limit.as_secs()
                );
                eprintln!("{msg}");
                use std::io::Write as _;
                let _ = std::io::stderr().write_all(msg.as_bytes());
                let _ = std::io::stderr().write_all(b"\n");
                on_fire();
            }
        });
        Self { done }
    }
}

impl Drop for RenderWatchdog {
    fn drop(&mut self) {
        self.done.store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

/// Locks the render mutex, **then** arms the guard, returning both.
///
/// The single ordering every render case uses: a case that queued behind others
/// must not spend its budget while waiting. The caller keeps the guard until it
/// has drawn; both the guard and the watchdog outlive the draw, so a hang there
/// is still caught.
pub(crate) fn lock_then_arm<'a>(
    mutex: &'a std::sync::Mutex<()>,
    name: &str,
    limit: Duration,
    on_fire: impl Fn() + Send + 'static,
) -> (std::sync::MutexGuard<'a, ()>, RenderWatchdog) {
    let guard = mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let watchdog = RenderWatchdog::arm_with(name, limit, on_fire);
    (guard, watchdog)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use std::sync::Arc;
    use std::time::Duration;

    use super::lock_then_arm;

    /// A case that queued behind another must not be killed for the wait.
    ///
    /// Case A holds the lock longer than B's limit, and B is already queued on
    /// it. Because B arms only after acquiring, A's hold never counts against B's
    /// budget. `fired` records a trip instead of aborting this binary. Arming
    /// before the lock is the defect: it sets `fired`, and the assertion fails
    /// with the same cause a healthy queued render used to die from.
    #[test]
    fn watchdog_does_not_fire_for_a_queued_case() {
        let mutex = Arc::new(std::sync::Mutex::new(()));
        let a_started = Arc::new(std::sync::atomic::AtomicBool::new(false));

        let started = Arc::clone(&a_started);
        let a_mutex = Arc::clone(&mutex);
        let a = std::thread::spawn(move || {
            let _guard = a_mutex.lock().expect("lock a");
            started.store(true, std::sync::atomic::Ordering::SeqCst);
            // Longer than B's 80 ms limit.
            std::thread::sleep(Duration::from_millis(160));
        });

        while !a_started.load(std::sync::atomic::Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(1));
        }

        let fired = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let fired_t = Arc::clone(&fired);
        {
            // B queues on the lock while A holds it; its limit is shorter than
            // the remaining hold, so arming early would fire.
            let (_guard_b, watchdog) = lock_then_arm(
                &mutex,
                "queued-case",
                Duration::from_millis(80),
                move || fired_t.store(true, std::sync::atomic::Ordering::SeqCst),
            );
            std::thread::sleep(Duration::from_millis(20));
            drop(watchdog);
        }
        a.join().expect("a joined");
        assert!(
            !fired.load(std::sync::atomic::Ordering::SeqCst),
            "the watchdog fired for a case that was only queued"
        );
    }

    /// A case that really hangs must trip the guard, promptly and with no
    /// queueing involved. `on_fire` stands in for `abort`, so this stays in
    /// process.
    #[test]
    fn watchdog_fires_for_a_stuck_case() {
        let mutex = std::sync::Mutex::new(());
        let fired = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let fired_t = Arc::clone(&fired);
        let (_guard, watchdog) =
            lock_then_arm(&mutex, "stuck-case", Duration::from_millis(40), move || {
                fired_t.store(true, std::sync::atomic::Ordering::SeqCst)
            });
        // Do not drop the guard: a stuck case never leaves its body.
        std::thread::sleep(Duration::from_millis(120));
        assert!(
            fired.load(std::sync::atomic::Ordering::SeqCst),
            "the watchdog did not fire for a stuck case"
        );
        std::mem::forget(watchdog);
    }
}
