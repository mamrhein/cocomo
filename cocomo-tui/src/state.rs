// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Application state: sessions, tabs, and the global atom.
//!
//! The TUI supports multiple comparison sessions, each displayed in its own
//! tab. The [`AppState`] owns all sessions and tracks which tab is active.
//! Each [`SessionView`] holds the full state for one comparison (paths,
//! results, filters, selection, etc.).

use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};

use cocomo_lib::{
    DirComparison, DirEntry, DirEntryStatus, Grammar, LineInfo, SyncOperation,
    SyncResult, TextDiff,
};
use ratatui_kit::{Atom, components::table::TableState};

/// Which pane currently has keyboard focus within a session.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) enum Focus {
    #[default]
    Left,
    Right,
}

/// The current input mode of a session.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) enum SessionMode {
    #[default]
    Normal,
    /// The user is typing a filter pattern.
    Filter,
    /// The user is typing a session name to save.
    SaveSession,
    /// The user is browsing saved sessions to load one.
    LoadSession,
}

/// The type of content a session displays.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum SessionType {
    /// Two-way folder comparison.
    DirCompare,
    /// Two-way text diff (stub for future implementation).
    TextCompare,
}

/// Actions that a session's key handler can request from the App component.
#[derive(Clone, Debug)]
pub(crate) enum SessionAction {
    /// Open a subdirectory comparison in a new session.
    OpenDir { left: PathBuf, right: PathBuf },
    /// Open a file comparison (text diff) in a new session.
    OpenFile { left: PathBuf, right: PathBuf },
    /// Navigate to the parent directories in a new session.
    GoUp,
    /// Reload the current session's comparison.
    Reload,
    /// Plan a sync (dry-run) and display the preview.
    PlanSync { operation: SyncOperation },
    /// Execute the planned sync.
    ExecuteSync,
    /// Save the current session config to a file.
    SaveSession { name: String },
    /// List saved session files for loading.
    ListSessions,
    /// Load a session config from a file.
    LoadSession { path: PathBuf },
    /// No action required.
    None,
}

/// Per-session state for a directory comparison.
///
/// Each tab in the TUI corresponds to one `SessionView`. It owns the
/// comparison results, selection state, filters, and UI preferences for
/// that session.
pub(crate) struct SessionView {
    /// Stable identity of this session.
    ///
    /// Spawned async tasks capture this id at spawn time and write their
    /// results back to the originating session, even if the user switches
    /// tabs while the task is running.
    pub id: usize,
    /// Type of content this session displays.
    pub session_type: SessionType,
    /// Display name shown in the tab bar.
    pub name: String,
    /// Left-side path (directory or file).
    pub left_path: PathBuf,
    /// Right-side path (directory or file).
    pub right_path: PathBuf,
    /// Directory comparison result, or `None` if still loading or not yet
    /// run.
    pub comparison: Option<DirComparison>,
    /// Text diff result, or `None` if not a text compare session.
    pub text_diff: Option<TextDiff>,
    /// Full left file content (for text compare sessions), indexed by line
    /// number (1-based).
    pub left_lines: Vec<LineInfo>,
    /// Full right file content (for text compare sessions), indexed by line
    /// number (1-based).
    pub right_lines: Vec<LineInfo>,
    /// Grammar used for syntax classification in text compare sessions.
    pub grammar: Grammar,
    /// Selected row in the entry table.
    pub table_state: TableState,
    /// Which pane has keyboard focus.
    pub focus: Focus,
    /// Current input mode.
    pub mode: SessionMode,
    /// Filter text input buffer.
    pub filter_input: String,
    /// Active filter pattern. `None` means no filter.
    pub active_filter: Option<String>,
    /// Hide identical entries.
    pub hide_same: bool,
    /// Compare file contents (vs. meta data only).
    pub compare_content: bool,
    /// Planned sync result for review. `Some` means sync preview is active.
    pub sync_planned: Option<SyncResult>,
    /// Sync operation currently configured.
    pub sync_operation: SyncOperation,
}

/// Monotonic counter assigning stable ids to sessions.
static NEXT_SESSION_ID: AtomicUsize = AtomicUsize::new(1);

impl SessionView {
    /// Create a new directory comparison session.
    pub fn new_dir_compare(
        left: PathBuf,
        right: PathBuf,
        compare_content: bool,
    ) -> Self {
        let name = Self::tab_title_for_paths(&left, &right);
        Self {
            id: NEXT_SESSION_ID.fetch_add(1, Ordering::Relaxed),
            session_type: SessionType::DirCompare,
            name,
            left_path: left,
            right_path: right,
            comparison: None,
            text_diff: None,
            left_lines: Vec::new(),
            right_lines: Vec::new(),
            grammar: Grammar::plain_text(),
            table_state: TableState::default(),
            focus: Focus::default(),
            mode: SessionMode::default(),
            filter_input: String::new(),
            active_filter: None,
            hide_same: false,
            compare_content,
            sync_planned: None,
            sync_operation: SyncOperation::default(),
        }
    }

    /// Create a new text comparison session.
    pub fn new_text_compare(
        left: PathBuf,
        right: PathBuf,
        left_lines: Vec<LineInfo>,
        right_lines: Vec<LineInfo>,
        text_diff: TextDiff,
        grammar: Grammar,
    ) -> Self {
        let name = Self::tab_title_for_paths(&left, &right);
        Self {
            id: NEXT_SESSION_ID.fetch_add(1, Ordering::Relaxed),
            session_type: SessionType::TextCompare,
            name,
            left_path: left,
            right_path: right,
            comparison: None,
            text_diff: Some(text_diff),
            left_lines,
            right_lines,
            grammar,
            table_state: TableState::default(),
            focus: Focus::default(),
            mode: SessionMode::default(),
            filter_input: String::new(),
            active_filter: None,
            hide_same: false,
            compare_content: true,
            sync_planned: None,
            sync_operation: SyncOperation::default(),
        }
    }

    /// Generate a short tab title from two paths.
    fn tab_title_for_paths(left: &Path, right: &Path) -> String {
        let left_name = left
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "…".to_string());
        let right_name = right
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "…".to_string());
        format!("{left_name} vs {right_name}")
    }

    /// Return the tab title for this session.
    pub fn tab_title(&self) -> &str {
        &self.name
    }

    /// Return the filtered entries for display.
    pub fn filtered_entries(&self) -> Vec<&DirEntry> {
        let Some(comparison) = &self.comparison else {
            return Vec::new();
        };

        comparison
            .entries
            .iter()
            .filter(|entry| {
                // Hide same entries if toggled.
                if self.hide_same {
                    match entry.status {
                        DirEntryStatus::Same | DirEntryStatus::SameBinary => {
                            return false;
                        }
                        _ => {}
                    }
                }

                // Apply name filter.
                if self.active_filter.as_ref().is_some_and(|filter| {
                    !entry.name.to_lowercase().contains(&filter.to_lowercase())
                }) {
                    return false;
                }

                true
            })
            .collect()
    }

    /// Return the path of the focused side.
    #[allow(unused)]
    pub fn active_path(&self) -> &PathBuf {
        match self.focus {
            Focus::Left => &self.left_path,
            Focus::Right => &self.right_path,
        }
    }
}

/// Top-level application state.
///
/// Manages multiple [`SessionView`] instances as tabs and tracks which tab
/// is currently active.
#[allow(dead_code)] // reload_pending and session_count are stubs for M5.2+.
pub(crate) struct AppState {
    /// All open session tabs.
    pub sessions: Vec<SessionView>,
    /// Index of the currently active tab.
    pub active_tab: usize,
    /// Session index that needs an async reload.
    pub reload_pending: Option<usize>,
    /// Index of the currently selected menu item.
    pub menu_selection: usize,
    /// Top-level error message to display in an `AlertModal`.
    ///
    /// Set when an operation that cannot be attributed to a specific session
    /// fails (e.g. the initial comparison launched from `main`).
    pub error_message: Option<String>,
}

/// Global reactive application state, subscribed to via
/// `use_atom(&APP_STATE)`.
///
/// Declared as a module-level static so that the controller and background
/// tasks can read or mutate it without a `Hooks` handle (via `.get()` /
/// `.set()`). Components subscribe with `hooks.use_atom(&APP_STATE)` to
/// receive re-render notifications on writes; they only ever read it — every
/// mutation goes through a function in [`crate::controller`].
#[allow(dead_code)] // subscribed via use_atom in the root App component (Step 2).
pub(crate) static APP_STATE: Atom<AppState> = Atom::new(AppState::new);

impl AppState {
    /// Create a new, empty application state.
    pub fn new() -> Self {
        Self {
            sessions: Vec::new(),
            active_tab: 0,
            reload_pending: None,
            menu_selection: 0,
            error_message: None,
        }
    }

    /// Add a session and switch to it.
    pub fn add_session(&mut self, session: SessionView) {
        self.sessions.push(session);
        self.active_tab = self.sessions.len() - 1;
    }

    /// Close the active tab.
    ///
    /// If there are other tabs, switch to the previous one (or the last one
    /// if the active tab was already the last). If this was the last tab,
    /// [`AppState::sessions`] becomes empty; the caller (event handler)
    /// detects this and triggers `exit()`.
    pub fn close_active(&mut self) {
        let len = self.sessions.len();
        if len == 0 {
            return;
        }

        self.sessions.remove(self.active_tab);

        if !self.sessions.is_empty() {
            // Clamp the active tab index.
            if self.active_tab >= self.sessions.len() {
                self.active_tab = self.sessions.len() - 1;
            }
        }
    }

    /// Switch to the next or previous tab.
    ///
    /// A positive `delta` moves forward; a negative `delta` moves backward.
    /// Wraps around at the boundaries.
    pub fn switch_tab(&mut self, delta: isize) {
        let len = self.sessions.len();
        if len <= 1 {
            return;
        }

        let delta = if delta == 0 { 1 } else { delta };
        self.active_tab = ((self.active_tab as isize + delta + len as isize)
            % len as isize) as usize;
    }

    /// Return a mutable reference to the active session, if any.
    pub fn active_mut(&mut self) -> Option<&mut SessionView> {
        self.sessions.get_mut(self.active_tab)
    }

    /// Return an immutable reference to the active session, if any.
    pub fn active(&self) -> Option<&SessionView> {
        self.sessions.get(self.active_tab)
    }

    /// Return a reference to the session with the given stable id.
    ///
    /// Unlike [`AppState::active`], this survives tab switches: spawned async
    /// tasks capture a session id at spawn time and write their results
    /// back to the originating session.
    pub fn session(&self, id: usize) -> Option<&SessionView> {
        self.sessions.iter().find(|s| s.id == id)
    }

    /// Return a mutable reference to the session with the given stable id.
    pub fn session_mut(&mut self, id: usize) -> Option<&mut SessionView> {
        self.sessions.iter_mut().find(|s| s.id == id)
    }

    /// Return the number of open sessions.
    #[allow(dead_code)] // Used by M5.4+ for sync progress display.
    pub fn session_count(&self) -> usize {
        self.sessions.len()
    }

    /// Return the index of the active tab.
    pub fn active_index(&self) -> usize {
        self.active_tab
    }
}
