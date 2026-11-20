use flume::Sender;
use std::thread;
use std::time::{Duration, Instant};

/// Events produced off-thread and applied on the GPUI main thread.
#[derive(Debug, Clone)]
pub enum UiEvent {
    Progress(f32),
    Tray(crate::app::InstanceAction),
}

/// Simulates a background download feeding progress into the UI.
///
/// Runs on a separate OS thread (never on the GPUI executor) and talks to the UI
/// through a channel, per PRD §9 rule 2. The transfer completes once and the thread
/// exits, so the app reaches a genuinely idle state (needed for the idle-RSS number).
pub fn spawn_fake_download(tx: Sender<UiEvent>) -> std::io::Result<()> {
    thread::Builder::new()
        .name("fake-download".to_string())
        .spawn(move || {
            let start = Instant::now();
            let total = Duration::from_millis(4200);
            loop {
                let elapsed = start.elapsed();
                let pct = (elapsed.as_secs_f32() / total.as_secs_f32() * 100.0).min(100.0);
                if tx.send(UiEvent::Progress(pct)).is_err() {
                    return;
                }
                if pct >= 100.0 {
                    return;
                }
                thread::sleep(Duration::from_millis(80));
            }
        })
        .map(|_handle| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn download_reports_progress_and_reaches_100_percent() {
        let (tx, rx) = flume::unbounded();
        spawn_fake_download(tx).expect("spawn test thread");

        let mut values = Vec::new();
        loop {
            match rx.recv_timeout(Duration::from_secs(6)) {
                Ok(UiEvent::Progress(pct)) => {
                    values.push(pct);
                    if pct >= 100.0 {
                        break;
                    }
                }
                Ok(UiEvent::Tray(_)) => panic!("download must not emit tray events"),
                Err(err) => panic!("download stopped early: {err}"),
            }
        }

        assert!(values.len() >= 2, "expected incremental updates");
        assert!(values.iter().all(|p| (0.0..=100.0).contains(p)));
        assert!((values.last().copied().unwrap_or_default() - 100.0).abs() < f32::EPSILON);
    }

    #[test]
    fn download_thread_exits_after_completion() {
        let (tx, rx) = flume::unbounded();
        spawn_fake_download(tx).expect("spawn test thread");
        let deadline = std::time::Instant::now() + Duration::from_secs(8);
        while std::time::Instant::now() < deadline {
            if rx.recv_timeout(Duration::from_secs(6)).is_err() {
                break;
            }
        }
        // The sender is dropped when the thread returns, so the channel must close.
        let closed = rx.recv_timeout(Duration::from_secs(2)).is_err();
        assert!(
            closed,
            "download thread should finish and close the channel"
        );
    }
}
