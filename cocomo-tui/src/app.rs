// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Application state and session management.
//!
//! The TUI supports multiple comparison sessions, each displayed in its own
//! tab. The [`App`] owns all sessions and tracks which tab is active. Each
//! [`SessionView`] holds the full state for one comparison (paths, results,
//! filters, selection, etc.).

use std::path::{Path, PathBuf};

use cocomo_lib::{
    CompareConfig, ContentCache, DirComparison, DirEntry, DirEntryStatus,
    LocalFs,
};
use ratatui::widgets::TableState;

/// Which pane currently has keyboard focus within a session.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Focus {
    #[default]
    Left,
    Right,
}

/// The current input mode of a session.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum AppMode {
    #[default]
    Normal,
    /// The user is typing a filter pattern.
    Filter,
}

/// The type of content a session displays.
#[allow(dead_code)] // TextCompare is a stub for M5.2.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SessionType {
    /// Two-way folder comparison.
    DirCompare,
    /// Two-way text diff (stub for future implementation).
    TextCompare,
}

/// Actions that a session's key handler can request from the main loop.
#[derive(Clone, Debug)]
pub enum SessionAction {
    /// Open a subdirectory comparison in a new session.
    OpenDir { left: PathBuf, right: PathBuf },
    /// Navigate to the parent directories in a new session.
    GoUp,
    /// Reload the current session's comparison.
    Reload,
    /// No action required.
    None,
}

/// Per-session state for a directory comparison.
///
/// Each tab in the TUI corresponds to one `SessionView`. It owns the
/// comparison results, selection state, filters, and UI preferences for
/// that session.
#[allow(dead_code)] // session_type and some fields are stubs for M5.2+.
pub struct SessionView {
    /// Type of content this session displays.
    pub session_type: SessionType,
    /// Display name shown in the tab bar.
    pub name: String,
    /// Left-side directory path.
    pub left_path: PathBuf,
    /// Right-side directory path.
    pub right_path: PathBuf,
    /// Comparison result, or `None` if still loading or not yet run.
    pub comparison: Option<DirComparison>,
    /// Selected row in the entry table.
    pub table_state: TableState,
    /// Which pane has keyboard focus.
    pub focus: Focus,
    /// Current input mode.
    pub mode: AppMode,
    /// Filter text input buffer.
    pub filter_input: String,
    /// Active filter pattern. `None` means no filter.
    pub active_filter: Option<String>,
    /// Hide identical entries.
    pub hide_same: bool,
    /// Compare file contents (vs. structure only).
    pub compare_files: bool,
    /// Errors encountered during comparison.
    pub errors: Vec<String>,
}

impl SessionView {
    /// Create a new directory comparison session.
    pub fn new_dir_compare(
        left: PathBuf,
        right: PathBuf,
        compare_files: bool,
    ) -> Self {
        let name = Self::tab_title_for_paths(&left, &right);
        Self {
            session_type: SessionType::DirCompare,
            name,
            left_path: left,
            right_path: right,
            comparison: None,
            table_state: TableState::default(),
            focus: Focus::default(),
            mode: AppMode::default(),
            filter_input: String::new(),
            active_filter: None,
            hide_same: false,
            compare_files,
            errors: Vec::new(),
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
    #[allow(dead_code)] // Used by M5.2+ for text diff sessions.
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
pub struct App {
    /// `false` when the app should exit.
    pub running: bool,
    /// All open session tabs.
    pub sessions: Vec<SessionView>,
    /// Index of the currently active tab.
    pub active_tab: usize,
    /// Session index that needs an async reload.
    pub reload_pending: Option<usize>,
}

impl App {
    /// Create a new, empty application state.
    pub fn new() -> Self {
        Self {
            running: true,
            sessions: Vec::new(),
            active_tab: 0,
            reload_pending: None,
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
    /// exit the application.
    pub fn close_active(&mut self) {
        let len = self.sessions.len();
        if len == 0 {
            return;
        }

        self.sessions.remove(self.active_tab);

        if self.sessions.is_empty() {
            self.running = false;
        } else {
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

/// Run a directory comparison and populate a [`SessionView`].
pub async fn run_comparison(session: &mut SessionView) -> anyhow::Result<()> {
    let fs = LocalFs::new("local");
    let cache = ContentCache::default_config();

    let config = if session.compare_files {
        CompareConfig::full()
    } else {
        CompareConfig::structure_only()
    };

    let comparison = cocomo_lib::compare_directories_node(
        &fs,
        &session.left_path,
        &session.right_path,
        &config,
        Some(&cache),
    )
    .await?;

    session.comparison = Some(comparison);
    session.table_state.select(Some(0));

    Ok(())
}
