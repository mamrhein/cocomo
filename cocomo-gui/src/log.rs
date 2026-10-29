// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Application log buffer and log pane visibility state.
//!
//! Holds the data behind the window's log pane: a global ring buffer of
//! leveled log lines that the state and session manager append to as
//! operations happen, plus the pane's open/closed flag. The pane itself is
//! rendered by [`crate::tabview::WindowRoot`]; keeping this module free of
//! rendering lets the buffer be unit-tested without a window.

use std::collections::VecDeque;

use gpui::{App, BorrowAppContext as _, Global, SharedString};

/// Default number of lines the buffer keeps before evicting the oldest.
pub const DEFAULT_CAPACITY: usize = 500;

/// Severity of a log line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogLevel {
    /// Routine operation result.
    Info,
    /// Recoverable problem the user should notice.
    // Constructed by later phases (e.g. sync conflicts); exercised in tests.
    #[allow(dead_code)]
    Warning,
    /// Failed operation.
    Error,
}

impl LogLevel {
    /// Short label rendered in front of the message in the log pane.
    pub fn label(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Warning => "warn",
            Self::Error => "error",
        }
    }
}

/// A single recorded log line.
#[derive(Clone, Debug)]
pub struct LogEntry {
    /// Severity of the line.
    pub level: LogLevel,
    /// The message text.
    pub message: SharedString,
}

/// Ring buffer of the most recent log lines, oldest first.
#[derive(Debug)]
pub struct LogBuffer {
    capacity: usize,
    entries: VecDeque<LogEntry>,
}

impl Default for LogBuffer {
    fn default() -> Self {
        Self::new(DEFAULT_CAPACITY)
    }
}

impl LogBuffer {
    /// Create an empty buffer that keeps at most `capacity` lines.
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            entries: VecDeque::new(),
        }
    }

    /// Append a line, evicting the oldest one when the buffer is full.
    pub fn push(&mut self, level: LogLevel, message: impl Into<SharedString>) {
        if self.capacity == 0 {
            return;
        }
        if self.entries.len() == self.capacity {
            self.entries.pop_front();
        }
        self.entries.push_back(LogEntry {
            level,
            message: message.into(),
        });
    }

    /// Iterate over the recorded lines, oldest first.
    pub fn entries(&self) -> impl Iterator<Item = &LogEntry> {
        self.entries.iter()
    }

    /// Number of recorded lines.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether nothing has been recorded yet.
    // Companion to `len`; exercised in tests, not yet needed by the shell.
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Drop all recorded lines.
    // Reserved for a future "Clear log" command; exercised in tests.
    #[allow(dead_code)]
    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

impl Global for LogBuffer {}

/// Whether the log pane is visible in the window shell.
///
/// A global (rather than `WindowRoot` state) so the View menu's toggle,
/// which is handled app-wide, can flip it without reaching into a window
/// entity. `WindowRoot` observes it and re-renders on change.
#[derive(Debug)]
pub struct LogPaneOpen(pub bool);

impl Global for LogPaneOpen {}

/// Append one line to the global [`LogBuffer`].
pub fn push(level: LogLevel, message: impl Into<SharedString>, cx: &mut App) {
    cx.update_global::<LogBuffer, _>(|buffer, _| buffer.push(level, message));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_keeps_order_below_capacity() {
        let mut buffer = LogBuffer::new(3);
        buffer.push(LogLevel::Info, "one");
        buffer.push(LogLevel::Warning, "two");
        buffer.push(LogLevel::Error, "three");
        let messages: Vec<&str> =
            buffer.entries().map(|e| e.message.as_str()).collect();
        assert_eq!(messages, ["one", "two", "three"]);
        let levels: Vec<LogLevel> =
            buffer.entries().map(|e| e.level).collect();
        assert_eq!(
            levels,
            [LogLevel::Info, LogLevel::Warning, LogLevel::Error]
        );
    }

    #[test]
    fn push_evicts_oldest_beyond_capacity() {
        let mut buffer = LogBuffer::new(2);
        buffer.push(LogLevel::Info, "one");
        buffer.push(LogLevel::Info, "two");
        buffer.push(LogLevel::Info, "three");
        let messages: Vec<&str> =
            buffer.entries().map(|e| e.message.as_str()).collect();
        assert_eq!(messages, ["two", "three"]);
        assert_eq!(buffer.len(), 2);
    }

    #[test]
    fn zero_capacity_buffer_stays_empty() {
        let mut buffer = LogBuffer::new(0);
        buffer.push(LogLevel::Info, "dropped");
        assert!(buffer.is_empty());
    }

    #[test]
    fn clear_empties_the_buffer() {
        let mut buffer = LogBuffer::default();
        buffer.push(LogLevel::Info, "one");
        buffer.clear();
        assert!(buffer.is_empty());
    }

    #[test]
    fn level_labels_are_stable() {
        assert_eq!(LogLevel::Info.label(), "info");
        assert_eq!(LogLevel::Warning.label(), "warn");
        assert_eq!(LogLevel::Error.label(), "error");
    }
}
