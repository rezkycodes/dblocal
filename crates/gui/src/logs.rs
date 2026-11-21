//! Log panel state: bounded, per-instance ring buffers plus the pure logic
//! behind the virtualised list (`F-LOG-01`).
//!
//! Two layers live here:
//!
//! - [`LogBuffer`] — one instance's lines, held as `SharedString` so the render
//!   path clones a pointer instead of rebuilding a `Vec<String>` every frame.
//!   It also owns the follow flag that decides whether new lines scroll into
//!   view.
//! - [`LogStore`] — a map keyed by `InstanceId` so each open tab shows its own
//!   lines. Opening a tab creates its entry; closing the tab drops it.
//!
//! None of this touches GPUI types, so it runs from the event bridge and is
//! unit-tested directly.

use std::collections::HashMap;

use dblocal_core::model::InstanceId;
use gpui_kit::SharedString;

/// How many lines the GUI keeps in memory (domain value, one source of truth).
pub fn capacity() -> usize {
    dblocal_core::consts::LOG_TAIL_LINES
}

/// How many lines "Salin log" puts on the clipboard (`F-LOG-01` AC3).
pub fn copy_count() -> usize {
    500
}

/// A bounded, followable ring buffer of log lines for one instance.
#[derive(Debug, Clone)]
pub struct LogBuffer {
    lines: std::collections::VecDeque<SharedString>,
    following: bool,
}

impl Default for LogBuffer {
    /// A fresh buffer follows: a newly opened instance shows the newest line.
    fn default() -> Self {
        Self {
            lines: std::collections::VecDeque::new(),
            following: true,
        }
    }
}

impl LogBuffer {
    /// Creates an empty buffer; new instances start in follow mode.
    pub fn new() -> Self {
        Self::default()
    }

    /// Replaces the buffer contents with a tail read from the service.
    pub fn replace_with(&mut self, lines: impl IntoIterator<Item = String>) {
        self.lines = lines.into_iter().map(SharedString::from).collect();
        self.trim();
    }

    /// Appends lines, dropping the oldest when over capacity.
    pub fn append(&mut self, lines: impl IntoIterator<Item = String>) {
        for line in lines {
            self.lines.push_back(SharedString::from(line));
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

    /// All retained lines, borrowed — the render path reads this directly, so
    /// nothing is cloned per frame.
    pub fn lines(&self) -> &std::collections::VecDeque<SharedString> {
        &self.lines
    }

    /// A specific retained line.
    pub fn line(&self, index: usize) -> Option<&SharedString> {
        self.lines.get(index)
    }

    /// Whether live-follow is on.
    pub fn is_following(&self) -> bool {
        self.following
    }

    /// Sets follow mode (follow = keep the newest line in view).
    pub fn set_following(&mut self, following: bool) {
        self.following = following;
    }

    /// The last `n` lines, oldest first, for "Salin log".
    pub fn copy_tail(&self, n: usize) -> Vec<String> {
        let skip = self.lines.len().saturating_sub(n);
        self.lines
            .iter()
            .skip(skip)
            .map(|line| line.to_string())
            .collect()
    }

    /// The "Salin log" payload, joined with newlines.
    pub fn copy_text(&self) -> String {
        self.copy_tail(copy_count()).join("\n")
    }
}

/// Per-instance log buffers for every open Logs tab.
#[derive(Debug, Default)]
pub struct LogStore {
    buffers: HashMap<InstanceId, LogBuffer>,
}

impl LogStore {
    /// Creates an empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether an instance's tab has a buffer.
    pub fn contains(&self, id: &InstanceId) -> bool {
        self.buffers.contains_key(id)
    }

    /// Opens (or returns) the buffer for an instance, in follow mode.
    pub fn open(&mut self, id: InstanceId) -> &mut LogBuffer {
        self.buffers.entry(id).or_default()
    }

    /// Replaces one instance's buffer with a tail read from the service.
    pub fn load(&mut self, id: &InstanceId, lines: impl IntoIterator<Item = String>) {
        self.buffers
            .entry(id.clone())
            .or_default()
            .replace_with(lines);
    }

    /// Appends lines to one instance's buffer, if its tab exists.
    pub fn append(&mut self, id: &InstanceId, lines: impl IntoIterator<Item = String>) {
        if let Some(buffer) = self.buffers.get_mut(id) {
            buffer.append(lines);
        }
    }

    /// Drops an instance's buffer.
    pub fn close(&mut self, id: &InstanceId) {
        self.buffers.remove(id);
    }

    /// The buffer for an instance, if its tab is open.
    pub fn get(&self, id: &InstanceId) -> Option<&LogBuffer> {
        self.buffers.get(id)
    }

    /// A mutable buffer for an instance, if its tab is open.
    pub fn get_mut(&mut self, id: &InstanceId) -> Option<&mut LogBuffer> {
        self.buffers.get_mut(id)
    }

    /// Number of open buffers.
    pub fn len(&self) -> usize {
        self.buffers.len()
    }

    /// Whether no tab is open.
    pub fn is_empty(&self) -> bool {
        self.buffers.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(name: &str) -> InstanceId {
        // Deterministic ids keyed by a fixed UUID v5-ish string per name.
        let hex = match name {
            "a" => "00000000-0000-4000-8000-00000000000a",
            "b" => "00000000-0000-4000-8000-00000000000b",
            _ => "00000000-0000-4000-8000-0000000000ff",
        };
        InstanceId::parse(hex).expect("valid uuid")
    }

    #[test]
    fn appending_beyond_capacity_drops_the_oldest() {
        let mut buffer = LogBuffer::new();
        buffer.append((0..capacity() + 5).map(|i| format!("line {i}")));
        assert_eq!(buffer.len(), capacity());
        assert_eq!(buffer.line(0).map(|s| s.as_ref()), Some("line 5"));
        assert_eq!(
            buffer.line(capacity() - 1).map(|s| s.as_ref()),
            Some(format!("line {}", capacity() + 4).as_str())
        );
    }

    #[test]
    fn replace_also_respects_capacity() {
        let mut buffer = LogBuffer::new();
        buffer.replace_with((0..capacity() + 10).map(|i| i.to_string()));
        assert_eq!(buffer.len(), capacity());
        assert_eq!(buffer.line(0).map(|s| s.as_ref()), Some("10"));
    }

    #[test]
    fn lines_are_shared_strings_not_rebuilt() {
        let mut buffer = LogBuffer::new();
        buffer.append(std::iter::once("hello".to_owned()));
        // `lines()` hands back the stored `SharedString` by reference, so the
        // render path pays for a clone of the pointer, never a new allocation.
        assert_eq!(buffer.lines().len(), 1);
        assert_eq!(buffer.lines()[0].as_ref(), "hello");
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
    fn empty_buffer_reports_itself() {
        let buffer = LogBuffer::new();
        assert!(buffer.is_empty());
        assert_eq!(buffer.copy_text(), "");
        assert!(buffer.line(0).is_none());
    }

    #[test]
    fn store_keeps_one_buffer_per_instance() {
        let mut store = LogStore::new();
        let a = id("a");
        let b = id("b");
        store.load(&a, vec!["a1".to_owned(), "a2".to_owned()]);
        store.load(&b, vec!["b1".to_owned()]);
        assert_eq!(store.len(), 2);
        assert_eq!(store.get(&a).expect("a").len(), 2);
        assert_eq!(
            store
                .get(&a)
                .and_then(|buf| buf.line(0))
                .map(|s| s.as_ref()),
            Some("a1")
        );
        assert_eq!(
            store
                .get(&b)
                .and_then(|buf| buf.line(0))
                .map(|s| s.as_ref()),
            Some("b1")
        );
    }

    #[test]
    fn store_append_does_not_leak_between_instances() {
        let mut store = LogStore::new();
        let a = id("a");
        let b = id("b");
        store.load(&a, vec!["a1".to_owned()]);
        store.load(&b, vec!["b1".to_owned()]);
        // A live event for B must not appear under A.
        store.append(&b, vec!["b2".to_owned()]);
        assert_eq!(store.get(&a).expect("a").len(), 1);
        assert_eq!(store.get(&b).expect("b").len(), 2);
    }

    #[test]
    fn store_append_for_a_closed_tab_is_ignored() {
        let mut store = LogStore::new();
        let a = id("a");
        // No buffer opened for `a`: the append is a silent no-op, exactly what
        // happens when a line arrives for an instance whose tab is closed.
        store.append(&a, vec!["late".to_owned()]);
        assert!(store.is_empty());
    }

    #[test]
    fn closing_a_tab_drops_its_buffer() {
        let mut store = LogStore::new();
        let a = id("a");
        store.load(&a, vec!["a1".to_owned()]);
        assert!(store.contains(&a));
        store.close(&a);
        assert!(!store.contains(&a));
        assert!(store.get(&a).is_none());
    }

    #[test]
    fn mutably_opening_creates_a_following_buffer() {
        let mut store = LogStore::new();
        let a = id("a");
        assert!(store.open(a.clone()).is_following());
        store.get_mut(&a).expect("open").set_following(false);
        assert!(!store.get(&a).expect("open").is_following());
    }
}
