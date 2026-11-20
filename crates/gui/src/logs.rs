//! Log panel state: a bounded ring buffer plus the pure logic behind the
//! virtualised list (`F-LOG-01`).
//!
//! The buffer is the source of truth the `uniform_list` renders over. Appending
//! is O(1) and never touches GPUI types, so it can run from the event bridge.

use std::collections::VecDeque;

/// How many lines the GUI keeps in memory (domain value, one source of truth).
pub fn capacity() -> usize {
    dblocal_core::consts::LOG_TAIL_LINES
}

/// How many lines "Salin log" puts on the clipboard (`F-LOG-01` AC3).
pub fn copy_count() -> usize {
    500
}

/// A bounded, followable ring buffer of log lines for one instance.
#[derive(Debug, Clone, Default)]
pub struct LogBuffer {
    lines: VecDeque<String>,
    following: bool,
    /// Instances whose live stream is currently watched by the service.
    watched: Option<String>,
}

impl LogBuffer {
    /// Creates an empty buffer; new instances start in follow mode.
    pub fn new() -> Self {
        Self {
            lines: VecDeque::new(),
            following: true,
            watched: None,
        }
    }

    /// Replaces the buffer contents with a tail read from the service.
    pub fn replace_with(&mut self, lines: impl IntoIterator<Item = String>) {
        self.lines = lines.into_iter().collect();
        self.trim();
    }

    /// Appends lines, dropping the oldest when over capacity.
    pub fn append(&mut self, lines: impl IntoIterator<Item = String>) {
        for line in lines {
            self.lines.push_back(line);
        }
        self.trim();
    }

    fn trim(&mut self) {
        let cap = capacity();
        while self.lines.len() > cap {
            self.lines.pop_front();
        }
    }

    /// Number of retained lines.
    pub fn len(&self) -> usize {
        self.lines.len()
    }

    /// Whether there is nothing to show.
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// A specific retained line.
    pub fn line(&self, index: usize) -> Option<&str> {
        self.lines.get(index).map(String::as_str)
    }

    /// Whether live-follow is on.
    pub fn is_following(&self) -> bool {
        self.following
    }

    /// Toggles follow mode.
    pub fn set_following(&mut self, following: bool) {
        self.following = following;
    }

    /// The instance whose stream is currently watched, if any.
    pub fn watched(&self) -> Option<&str> {
        self.watched.as_deref()
    }

    /// Records the watched instance (the caller issues `watch_logs` separately).
    pub fn set_watched(&mut self, instance: Option<String>) {
        self.watched = instance;
    }

    /// The last `n` lines, oldest first, for "Salin log".
    pub fn copy_tail(&self, n: usize) -> Vec<String> {
        let skip = self.lines.len().saturating_sub(n);
        self.lines.iter().skip(skip).cloned().collect()
    }

    /// The "Salin log" payload, joined with newlines.
    pub fn copy_text(&self) -> String {
        self.copy_tail(copy_count()).join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appending_beyond_capacity_drops_the_oldest() {
        let mut buffer = LogBuffer::new();
        buffer.append((0..capacity() + 5).map(|i| format!("line {i}")));
        assert_eq!(buffer.len(), capacity());
        assert_eq!(buffer.line(0), Some("line 5"));
        assert_eq!(
            buffer.line(capacity() - 1),
            Some(format!("line {}", capacity() + 4).as_str())
        );
    }

    #[test]
    fn replace_also_respects_capacity() {
        let mut buffer = LogBuffer::new();
        buffer.replace_with((0..capacity() + 10).map(|i| i.to_string()));
        assert_eq!(buffer.len(), capacity());
        assert_eq!(buffer.line(0), Some("10"));
    }

    #[test]
    fn follow_mode_defaults_on_and_can_pause() {
        let mut buffer = LogBuffer::new();
        assert!(buffer.is_following());
        buffer.set_following(false);
        assert!(!buffer.is_following());
    }

    #[test]
    fn copy_tail_returns_the_last_lines_in_order() {
        let mut buffer = LogBuffer::new();
        buffer.append((0..10).map(|i| i.to_string()));
        assert_eq!(buffer.copy_tail(3), vec!["7", "8", "9"]);
        assert_eq!(
            buffer.copy_text(),
            (0..10)
                .map(|i| i.to_string())
                .collect::<Vec<_>>()
                .join("\n")
        );
    }

    #[test]
    fn copy_count_is_capped_for_humans() {
        assert_eq!(copy_count(), 500);
        let mut buffer = LogBuffer::new();
        buffer.append((0..800).map(|i| i.to_string()));
        let copied = buffer.copy_tail(copy_count());
        assert_eq!(copied.len(), 500);
        assert_eq!(copied.first().map(String::as_str), Some("300"));
        assert_eq!(copied.last().map(String::as_str), Some("799"));
    }

    #[test]
    fn watched_instance_round_trips() {
        let mut buffer = LogBuffer::new();
        assert!(buffer.watched().is_none());
        buffer.set_watched(Some("app-dev".to_owned()));
        assert_eq!(buffer.watched(), Some("app-dev"));
        buffer.set_watched(None);
        assert!(buffer.watched().is_none());
    }

    #[test]
    fn empty_buffer_reports_itself() {
        let buffer = LogBuffer::new();
        assert!(buffer.is_empty());
        assert_eq!(buffer.copy_text(), "");
        assert!(buffer.line(0).is_none());
    }
}
