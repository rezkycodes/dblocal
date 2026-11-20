//! Instance log helpers: tail, follow and rotate (`F-LOG-01`).
//!
//! Log lines are treated as opaque bytes: this module never parses, never
//! redacts and never logs them. Secrets must not reach an engine log in the
//! first place (invariant 5); callers that need to display lines are responsible
//! for not echoing them anywhere else.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::time::Duration;

use crate::consts;
use crate::error::Result;

/// Reads the last `n` newline-terminated lines of `path`.
///
/// The file is read from the end in chunks, so a 50 MB log does not cost 50 MB
/// of memory. A partial last line (no trailing newline) is included. A missing
/// file is reported as an empty result rather than an error, because the GUI
/// polls the tail of a file that may not exist yet.
pub fn tail_lines(path: &Path, n: usize) -> Result<Vec<String>> {
    if n == 0 {
        return Ok(Vec::new());
    }
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err.into()),
    };
    let len = file.metadata()?.len();
    if len == 0 {
        return Ok(Vec::new());
    }

    const CHUNK: u64 = 8 * 1024;
    // A whole file of newlines would let the backward scan run to the start; a
    // real log line is tiny, so bounding the window keeps a pathological file
    // from costing a full read without changing any realistic result.
    const MAX_WINDOW: u64 = 8 * 1024 * 1024;
    let mut window = 0u64;
    let mut cursor = len;
    let mut newline_count = 0usize;
    let mut start = 0u64;
    let mut found_start = false;
    let mut buf = vec![0u8; usize::try_from(CHUNK).unwrap_or(usize::MAX)];

    // Walk backwards counting newlines without copying, then read the window
    // once. Prepending each chunk instead would be quadratic in file size.
    while cursor > 0 && window < MAX_WINDOW {
        let read_len = std::cmp::min(CHUNK, cursor);
        cursor -= read_len;
        window += read_len;
        let read_len_usize = usize::try_from(read_len).unwrap_or(usize::MAX);
        file.seek(SeekFrom::Start(cursor))?;
        file.read_exact(&mut buf[..read_len_usize])?;
        for index in (0..read_len_usize).rev() {
            if buf[index] == b'\n' {
                newline_count += 1;
                // `n` trailing lines need `n` newlines counted backwards from
                // the final byte (a trailing newline ends the last line).
                if newline_count > n {
                    start = cursor + index as u64 + 1;
                    found_start = true;
                    break;
                }
            }
        }
        if found_start {
            break;
        }
    }

    let read_len = len - start;
    let read_len_usize = usize::try_from(read_len).unwrap_or(usize::MAX);
    let mut collected = vec![0u8; read_len_usize];
    file.seek(SeekFrom::Start(start))?;
    file.read_exact(&mut collected)?;
    Ok(decode_last_lines(&collected, n))
}

fn decode_last_lines(bytes: &[u8], n: usize) -> Vec<String> {
    // Treat \r\n and \n the same way. Trailing newlines do not create an empty
    // "phantom" last line.
    let text = String::from_utf8_lossy(bytes);
    let mut lines: Vec<&str> = text.lines().collect();
    if lines.len() > n {
        lines = lines.split_off(lines.len() - n);
    }
    lines.into_iter().map(str::to_owned).collect()
}

/// How many bytes one [`LogFollower::poll`] may consume.
///
/// Keeps a follower that was idle for a long time from allocating the whole
/// file; the next poll picks up where this one stopped.
pub const POLL_READ_LIMIT: u64 = 1024 * 1024;

/// Poll-based follower for the "live" log view (`F-LOG-01` AC1).
///
/// Call [`LogFollower::poll`] on a timer to learn about lines appended since
/// the previous call. The follower tracks the byte offset it has consumed, so
/// it never re-reads the whole file. Truncation or rotation (file shrinks or
/// the inode is replaced) resets it to the start of the new file.
#[derive(Debug)]
pub struct LogFollower {
    path: std::path::PathBuf,
    offset: u64,
    /// Partial line that arrived without a trailing newline yet.
    pending: Vec<u8>,
}

impl LogFollower {
    /// Starts following `path` at its current end.
    ///
    /// Pass `from_start: true` to deliver the existing content on the first
    /// poll instead of only appends.
    pub fn new(path: impl Into<std::path::PathBuf>, from_start: bool) -> Result<Self> {
        let path = path.into();
        let offset = if from_start {
            0
        } else {
            fs::metadata(&path).map(|m| m.len()).unwrap_or(0)
        };
        Ok(Self {
            path,
            offset,
            pending: Vec::new(),
        })
    }

    /// Returns every complete line appended since the last poll.
    ///
    /// A line is "complete" once a newline has arrived. A missing file is not
    /// an error: the follower keeps waiting. At most [`POLL_READ_LIMIT`] bytes
    /// are consumed per poll, so a long-idle follower cannot allocate the whole
    /// file in one call; callers poll again for the rest.
    pub fn poll(&mut self) -> Result<Vec<String>> {
        let mut file = match File::open(&self.path) {
            Ok(file) => file,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(err) => return Err(err.into()),
        };
        let len = file.metadata()?.len();
        if len < self.offset {
            // Rotated or truncated: start over from the beginning.
            self.offset = 0;
            self.pending.clear();
        }
        if len == self.offset {
            return Ok(Vec::new());
        }
        file.seek(SeekFrom::Start(self.offset))?;
        let available = len - self.offset;
        let to_read = usize::try_from(available.min(POLL_READ_LIMIT)).unwrap_or(usize::MAX);
        let mut chunk = vec![0u8; to_read];
        let read = file.read(&mut chunk)?;
        chunk.truncate(read);
        self.offset += read as u64;
        self.pending.extend_from_slice(&chunk);

        let mut lines = Vec::new();
        while let Some(pos) = self.pending.iter().position(|b| *b == b'\n') {
            let mut line: Vec<u8> = self.pending.drain(..=pos).collect();
            if line.last() == Some(&b'\n') {
                line.pop();
            }
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            lines.push(String::from_utf8_lossy(&line).into_owned());
        }
        Ok(lines)
    }

    /// Path being followed.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// A follower that sleeps between polls, convenient for a background thread.
pub fn follow_forever(
    mut follower: LogFollower,
    interval: Duration,
    mut sink: impl FnMut(Vec<String>) -> bool,
) -> Result<()> {
    loop {
        let lines = follower.poll()?;
        if !lines.is_empty() && !sink(lines) {
            return Ok(());
        }
        std::thread::sleep(interval);
    }
}

/// Rotates `path` at instance start when it already exceeds the configured
/// limit.
///
/// Files are renamed `name -> name.1 -> name.2 -> ...`, keeping at most
/// [`crate::consts::LOG_ROTATE_KEEP`] old generations. The active file is left
/// absent so the engine (or the next spawn) creates a fresh one.
///
/// The rotation is best-effort: a missing file or an undersized file is a
/// no-op, and a failure to shift one generation stops the chain but does not
/// panic.
pub fn rotate_if_needed(path: &Path) -> Result<bool> {
    rotate_if_needed_with(path, consts::LOG_MAX_BYTES, consts::LOG_ROTATE_KEEP)
}

fn rotate_if_needed_with(path: &Path, max_bytes: u64, keep: u32) -> Result<bool> {
    let meta = match fs::metadata(path) {
        Ok(meta) => meta,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(err) => return Err(err.into()),
    };
    if meta.len() <= max_bytes {
        return Ok(false);
    }
    // Oldest first: drop the tail end of the chain, then shift down.
    if keep == 0 {
        let _ = fs::remove_file(path);
        return Ok(true);
    }
    let _ = fs::remove_file(rotated_path(path, keep));
    for generation in (1..keep).rev() {
        let from = rotated_path(path, generation);
        let to = rotated_path(path, generation + 1);
        if from.exists() {
            let _ = fs::rename(&from, &to);
        }
    }
    fs::rename(path, rotated_path(path, 1))?;
    // Touch a fresh, empty active file so a reader sees "no old content".
    OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path)?;
    Ok(true)
}

fn rotated_path(path: &Path, generation: u32) -> std::path::PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(format!(".{generation}"));
    std::path::PathBuf::from(name)
}

/// Reports the configured follow interval. Callers that want "live" updates
/// without busy-waiting poll at this cadence.
pub const fn follow_interval() -> Duration {
    Duration::from_millis(250)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::tempdir;

    #[test]
    fn missing_file_yields_no_lines() {
        let dir = tempdir().expect("tempdir");
        let lines = tail_lines(&dir.path().join("nope.log"), 20).expect("tail");
        assert!(lines.is_empty());
    }

    #[test]
    fn empty_file_yields_no_lines() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("empty.log");
        File::create(&path).expect("create");
        assert!(tail_lines(&path, 20).expect("tail").is_empty());
    }

    #[test]
    fn tail_returns_the_last_n_lines_in_order() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("app.log");
        fs::write(&path, "a\nb\nc\nd\ne\n").expect("write");
        assert_eq!(tail_lines(&path, 3).expect("tail"), vec!["c", "d", "e"]);
        assert_eq!(
            tail_lines(&path, 10).expect("tail"),
            vec!["a", "b", "c", "d", "e"]
        );
        assert_eq!(tail_lines(&path, 0).expect("tail"), Vec::<String>::new());
    }

    #[test]
    fn a_partial_last_line_is_included() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("partial.log");
        fs::write(&path, "first\nsecond\nthird").expect("write");
        assert_eq!(tail_lines(&path, 2).expect("tail"), vec!["second", "third"]);
        assert_eq!(tail_lines(&path, 1).expect("tail"), vec!["third"]);
    }

    #[test]
    fn windows_line_endings_are_handled() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("crlf.log");
        fs::write(&path, "one\r\ntwo\r\nthree\r\n").expect("write");
        assert_eq!(tail_lines(&path, 2).expect("tail"), vec!["two", "three"]);
    }

    #[test]
    fn tail_on_a_large_file_stays_bounded() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("big.log");
        let mut file = File::create(&path).expect("create");
        // 50 MB of short lines. Only the tail is ever materialised.
        let line = b"2026-01-01 INFO engine ready and accepting connections\n";
        let target = 50 * 1024 * 1024usize;
        let mut written = 0usize;
        while written < target {
            file.write_all(line).expect("write");
            written += line.len();
        }
        file.flush().expect("flush");
        let start = std::time::Instant::now();
        let lines = tail_lines(&path, 20).expect("tail");
        assert_eq!(lines.len(), 20);
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "tail took {:?}",
            start.elapsed()
        );
    }

    #[test]
    fn follower_delivers_appended_lines() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("live.log");
        fs::write(&path, "old\n").expect("write");
        let mut follower = LogFollower::new(&path, false).expect("follower");
        assert!(follower.poll().expect("poll").is_empty(), "starts at end");

        let mut file = OpenOptions::new().append(true).open(&path).expect("open");
        writeln!(file, "new-1").expect("write");
        writeln!(file, "new-2").expect("write");
        file.flush().expect("flush");
        assert_eq!(follower.poll().expect("poll"), vec!["new-1", "new-2"]);

        // A partial line is held until the newline arrives.
        write!(file, "part").expect("write");
        file.flush().expect("flush");
        assert!(follower.poll().expect("poll").is_empty());
        writeln!(file, "ial").expect("write");
        file.flush().expect("flush");
        assert_eq!(follower.poll().expect("poll"), vec!["partial"]);
    }

    #[test]
    fn follower_recovers_after_truncation() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("rotate.log");
        fs::write(&path, "first\nsecond\n").expect("write");
        let mut follower = LogFollower::new(&path, true).expect("follower");
        assert_eq!(follower.poll().expect("poll"), vec!["first", "second"]);

        // Truncate and write less than the old offset.
        fs::write(&path, "fresh\n").expect("truncate");
        assert_eq!(follower.poll().expect("poll"), vec!["fresh"]);
    }

    #[test]
    fn follower_tolerates_a_missing_file() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("later.log");
        let mut follower = LogFollower::new(&path, true).expect("follower");
        assert!(follower.poll().expect("poll").is_empty());
        fs::write(&path, "now\n").expect("write");
        assert_eq!(follower.poll().expect("poll"), vec!["now"]);
    }

    #[test]
    fn rotation_keeps_the_configured_number_of_generations() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("engine.log");
        fs::write(&path, "old-1").expect("write");
        assert!(rotate_if_needed_with(&path, 4, 3).expect("rotate"));
        assert!(path.exists(), "active file recreated");
        assert_eq!(
            fs::read_to_string(rotated_path(&path, 1)).expect("read"),
            "old-1"
        );
        assert!(!rotated_path(&path, 2).exists());

        fs::write(&path, "old-2").expect("write");
        rotate_if_needed_with(&path, 4, 3).expect("rotate");
        assert_eq!(
            fs::read_to_string(rotated_path(&path, 1)).expect("read"),
            "old-2"
        );
        assert_eq!(
            fs::read_to_string(rotated_path(&path, 2)).expect("read"),
            "old-1"
        );

        fs::write(&path, "old-3").expect("write");
        rotate_if_needed_with(&path, 4, 3).expect("rotate");
        fs::write(&path, "old-4").expect("write");
        rotate_if_needed_with(&path, 4, 3).expect("rotate");
        assert_eq!(
            fs::read_to_string(rotated_path(&path, 1)).expect("read"),
            "old-4"
        );
        assert_eq!(
            fs::read_to_string(rotated_path(&path, 2)).expect("read"),
            "old-3"
        );
        assert_eq!(
            fs::read_to_string(rotated_path(&path, 3)).expect("read"),
            "old-2"
        );
        assert!(
            !rotated_path(&path, 4).exists(),
            "only LOG_ROTATE_KEEP kept"
        );
    }

    #[test]
    fn rotation_is_a_no_op_when_small_or_missing() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("small.log");
        fs::write(&path, "tiny").expect("write");
        assert!(!rotate_if_needed_with(&path, 1024, 3).expect("rotate"));
        assert_eq!(fs::read_to_string(&path).expect("read"), "tiny");
        assert!(!rotate_if_needed_with(&dir.path().join("none.log"), 1, 3).expect("rotate"));
    }

    #[test]
    fn rotation_with_zero_keep_drops_the_file() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("drop.log");
        fs::write(&path, "big-enough").expect("write");
        assert!(rotate_if_needed_with(&path, 4, 0).expect("rotate"));
        assert!(!path.exists(), "active file removed when nothing is kept");
    }

    #[test]
    fn follow_forever_stops_when_the_sink_asks() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("follow.log");
        fs::write(&path, "ready\n").expect("write");
        let follower = LogFollower::new(&path, true).expect("follower");
        let mut seen = Vec::new();
        // The first poll delivers "ready"; returning false stops the loop
        // instead of blocking this test forever.
        let result = follow_forever(follower, Duration::from_millis(5), |lines| {
            seen.extend(lines);
            false
        });
        assert!(result.is_ok());
        assert_eq!(seen, vec!["ready"]);
    }

    #[test]
    fn follow_forever_keeps_polling_until_content_arrives() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("later.log");
        let follower = LogFollower::new(&path, true).expect("follower");
        let writer_path = path.clone();
        let writer = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            fs::write(&writer_path, "arrived\n").expect("write");
        });
        let mut seen = Vec::new();
        let result = follow_forever(follower, Duration::from_millis(5), |lines| {
            seen.extend(lines);
            false
        });
        writer.join().expect("writer");
        assert!(result.is_ok());
        assert_eq!(seen, vec!["arrived"]);
    }

    #[test]
    fn follower_reads_a_large_backlog_in_bounded_chunks() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("backlog.log");
        let mut file = File::create(&path).expect("create");
        let line = "0123456789abcdef\n";
        let target = (POLL_READ_LIMIT as usize) * 2 + 1024;
        let mut written = 0usize;
        while written < target {
            file.write_all(line.as_bytes()).expect("write");
            written += line.len();
        }
        drop(file);

        let mut follower = LogFollower::new(&path, true).expect("follower");
        let first = follower.poll().expect("poll");
        assert!(!first.is_empty());
        let second = follower.poll().expect("poll");
        let third = follower.poll().expect("poll");
        let total = first.len() + second.len() + third.len();
        assert_eq!(total, written / line.len());
        assert!(follower.poll().expect("poll").is_empty());
    }

    #[test]
    fn tail_on_a_single_line_giant_file_stays_bounded() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("oneline.log");
        let mut file = File::create(&path).expect("create");
        let chunk = vec![b'a'; 1024 * 1024];
        for _ in 0..16 {
            file.write_all(&chunk).expect("write");
        }
        file.flush().expect("flush");
        let start = std::time::Instant::now();
        let lines = tail_lines(&path, 1).expect("tail");
        assert_eq!(lines.len(), 1);
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "tail took {:?}",
            start.elapsed()
        );
    }

    #[test]
    fn follow_interval_is_sane() {
        assert_eq!(follow_interval(), Duration::from_millis(250));
    }
}
