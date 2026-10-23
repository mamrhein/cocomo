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
//! Each [`SessionView`] splits its state by role: a [`SessionCore`] with the
//! domain identity (id, name, paths), a [`SessionContent`] that encodes
//! whether the session compares directories or text (so no separate session
//! type flag can drift from the data), and a [`UiState`] with the view state
//! shared by both content types.

use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};

use cocomo_lib::{
    DirComparison, DirEntry, DirEntryStatus, Grammar, LineInfo, SyncOperation,
    SyncResult, TextDiff,
};
use ratatui_kit::{Atom, components::table::TableState};

/// The current input mode of a session.
///
/// Each text-entry mode owns its input buffers, so a buffer cannot leak
/// from one mode into another: leaving a mode drops its state with it.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum SessionMode {
    #[default]
    Normal,
    /// The user is typing a filter pattern (buffer:
    /// [`UiState::filter_input`]).
    Filter,
    /// The user is typing a session name to save.
    SaveSession { name: String },
    /// The user is picking a saved session to load. `saved_sessions` is
    /// filled asynchronously by the `ListSessions` action.
    LoadSession {
        name: String,
        saved_sessions: Vec<String>,
    },
}

impl SessionMode {
    /// Returns `true` when the user is typing into a text field.
    ///
    /// While editing, printable keys belong to the input component's layer;
    /// the shell and session handlers must not claim them (e.g. `q` must
    /// not quit).
    pub fn is_editing(&self) -> bool {
        matches!(
            self,
            Self::Filter | Self::SaveSession { .. } | Self::LoadSession { .. }
        )
    }
}

/// Actions that a session's key handler can request from the App component.
#[derive(Clone, Debug, PartialEq)]
pub enum SessionAction {
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

/// Per-session state for one comparison tab.
///
/// Each tab in the TUI corresponds to one `SessionView`. Its state is split
/// by role: [`SessionCore`] holds the domain identity (id, display name,
/// compared paths), [`SessionContent`] holds the comparison-specific data
/// (a directory comparison or a text diff, never both), and [`UiState`]
/// holds the view state shared by both content types.
pub struct SessionView {
    /// Domain identity of this session.
    pub core: SessionCore,
    /// The comparison this session displays.
    pub content: SessionContent,
    /// View state shared by both content types.
    pub ui: UiState,
}

/// The domain identity of a session: what is compared and how it is named.
pub struct SessionCore {
    /// Stable identity of this session.
    ///
    /// Spawned async tasks capture this id at spawn time and write their
    /// results back to the originating session, even if the user switches
    /// tabs while the task is running.
    pub id: usize,
    /// Display name shown in the tab bar.
    pub name: String,
    /// Left-side path (directory or file).
    pub left_path: PathBuf,
    /// Right-side path (directory or file).
    pub right_path: PathBuf,
}

/// The content of a session.
///
/// The variant encodes the session type, so no separate type flag can drift
/// from the data that is actually present, and dir-only fields are simply
/// absent from text sessions (and vice versa).
pub enum SessionContent {
    /// Two-way folder comparison.
    Dir(DirView),
    /// Two-way text diff.
    Text(TextDiffView),
}

impl SessionContent {
    /// Returns `true` for a directory comparison session.
    pub fn is_dir(&self) -> bool {
        matches!(self, Self::Dir(_))
    }

    /// Returns `true` for a text diff session.
    pub fn is_text(&self) -> bool {
        matches!(self, Self::Text(_))
    }
}

/// Directory comparison state: the result plus the settings that drive it.
pub struct DirView {
    /// Directory comparison result, or `None` if still loading or not yet
    /// run.
    pub comparison: Option<DirComparison>,
    /// Compare file contents (vs. meta data only).
    pub compare_content: bool,
    /// Hide identical entries.
    pub hide_same: bool,
    /// Sync workflow stage, or `None` when no sync is planned or executed.
    pub sync: Option<SyncStage>,
    /// Sync operation currently configured.
    pub sync_operation: SyncOperation,
}

/// The lifecycle stage of a sync operation.
///
/// The variant distinguishes a dry-run plan from an executed sync, so the
/// status label reads from one match instead of inferring the stage from
/// the internals of the [`SyncResult`].
#[derive(Clone, Debug)]
pub enum SyncStage {
    /// A dry-run plan awaiting review or execution.
    Planned(SyncResult),
    /// The planned sync was executed.
    Executed(SyncResult),
}

impl SyncStage {
    /// Return the underlying [`SyncResult`], regardless of stage.
    pub fn result(&self) -> &SyncResult {
        match self {
            Self::Planned(result) | Self::Executed(result) => result,
        }
    }
}

/// Text diff state: the diff result plus the file-content cache.
pub struct TextDiffView {
    /// Text diff result, or `None` while loading.
    pub text_diff: Option<TextDiff>,
    /// Full left file content, in display order (line numbers are 1-based).
    pub left_lines: Vec<LineInfo>,
    /// Full right file content, in display order (line numbers are 1-based).
    pub right_lines: Vec<LineInfo>,
    /// Grammar used for syntax classification.
    pub grammar: Grammar,
}

/// View state shared by both session content types.
#[derive(Default)]
pub struct UiState {
    /// Selected row in the entry table.
    pub table_state: TableState,
    /// Current input mode (owns the save/load name buffers).
    pub mode: SessionMode,
    /// Filter text input buffer, used only while [`SessionMode::Filter`]
    /// is active.
    pub filter_input: String,
    /// Active filter pattern. `None` means no filter.
    pub active_filter: Option<String>,
}

/// Monotonic counter assigning stable ids to sessions.
static NEXT_SESSION_ID: AtomicUsize = AtomicUsize::new(1);

impl SessionCore {
    /// Create the core for a session comparing `left` with `right`, deriving
    /// the tab title from the two paths.
    fn new(left_path: PathBuf, right_path: PathBuf) -> Self {
        let name = Self::tab_title_for_paths(&left_path, &right_path);
        Self {
            id: NEXT_SESSION_ID.fetch_add(1, Ordering::Relaxed),
            name,
            left_path,
            right_path,
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
}

impl SessionView {
    /// Create a new directory comparison session.
    pub fn new_dir_compare(
        left: PathBuf,
        right: PathBuf,
        compare_content: bool,
    ) -> Self {
        Self {
            core: SessionCore::new(left, right),
            content: SessionContent::Dir(DirView {
                comparison: None,
                compare_content,
                hide_same: false,
                sync: None,
                sync_operation: SyncOperation::default(),
            }),
            ui: UiState::default(),
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
        Self {
            core: SessionCore::new(left, right),
            content: SessionContent::Text(TextDiffView {
                text_diff: Some(text_diff),
                left_lines,
                right_lines,
                grammar,
            }),
            ui: UiState::default(),
        }
    }

    /// Return the directory comparison state, or `None` for a text session.
    pub fn dir(&self) -> Option<&DirView> {
        match &self.content {
            SessionContent::Dir(dir) => Some(dir),
            SessionContent::Text(_) => None,
        }
    }

    /// Return the directory comparison state mutably, or `None` for a text
    /// session.
    pub fn dir_mut(&mut self) -> Option<&mut DirView> {
        match &mut self.content {
            SessionContent::Dir(dir) => Some(dir),
            SessionContent::Text(_) => None,
        }
    }

    /// Return the text diff state, or `None` for a directory session.
    pub fn text(&self) -> Option<&TextDiffView> {
        match &self.content {
            SessionContent::Text(text) => Some(text),
            SessionContent::Dir(_) => None,
        }
    }

    /// Return the text diff state mutably, or `None` for a directory
    /// session.
    pub fn text_mut(&mut self) -> Option<&mut TextDiffView> {
        match &mut self.content {
            SessionContent::Text(text) => Some(text),
            SessionContent::Dir(_) => None,
        }
    }

    /// Set the directory comparison result and reset the table selection to
    /// the first row.
    ///
    /// `None` puts the session into the loading state, `Some(result)` shows
    /// a fresh comparison. The selection is reset in both cases so content
    /// and selection can never disagree; callers no longer pair the
    /// assignment with a manual `select(Some(0))`. A no-op on a text
    /// session, which has no directory comparison.
    pub fn set_comparison(&mut self, comparison: Option<DirComparison>) {
        if let Some(dir) = self.dir_mut() {
            dir.comparison = comparison;
            self.ui.table_state.select(Some(0));
        }
    }

    /// Return the tab title for this session.
    pub fn tab_title(&self) -> &str {
        &self.core.name
    }

    /// Number of table rows this session currently displays.
    ///
    /// Directory sessions show the filtered entries; text sessions show one
    /// row per line of the merged side-by-side view. Navigation must clamp
    /// against this count, not against [`SessionView::filtered_entries`],
    /// which is empty for text sessions.
    pub fn row_count(&self) -> usize {
        match &self.content {
            SessionContent::Dir(_) => self.filtered_entries().len(),
            SessionContent::Text(text) => {
                std::cmp::max(text.left_lines.len(), text.right_lines.len())
            }
        }
    }

    /// Return the filtered entries for display.
    ///
    /// Empty for text sessions, which have no directory entries.
    pub fn filtered_entries(&self) -> Vec<&DirEntry> {
        let Some(dir) = self.dir() else {
            return Vec::new();
        };
        let Some(comparison) = dir.comparison.as_ref() else {
            return Vec::new();
        };

        comparison
            .entries
            .iter()
            .filter(|entry| {
                // Hide same entries if toggled.
                if dir.hide_same {
                    match entry.status {
                        DirEntryStatus::Same | DirEntryStatus::SameBinary => {
                            return false;
                        }
                        _ => {}
                    }
                }

                // Apply name filter.
                if self.ui.active_filter.as_ref().is_some_and(|filter| {
                    !entry.name.to_lowercase().contains(&filter.to_lowercase())
                }) {
                    return false;
                }

                true
            })
            .collect()
    }
}

/// Top-level application state.
///
/// Manages multiple [`SessionView`] instances as tabs and tracks which tab
/// is currently active.
#[derive(Default)]
pub struct AppState {
    /// All open session tabs.
    pub sessions: Vec<SessionView>,
    /// Index of the currently active tab.
    pub active_tab: usize,
    /// Index of the currently selected menu item.
    pub menu_selection: usize,
    /// Top-level error message to display in an `AlertModal`.
    ///
    /// Set when an operation that cannot be attributed to a specific session
    /// fails (e.g. the initial comparison launched at startup).
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
        self.sessions.iter().find(|s| s.core.id == id)
    }

    /// Return a mutable reference to the session with the given stable id.
    pub fn session_mut(&mut self, id: usize) -> Option<&mut SessionView> {
        self.sessions.iter_mut().find(|s| s.core.id == id)
    }

    /// Return the index of the active tab.
    pub fn active_index(&self) -> usize {
        self.active_tab
    }
}

#[cfg(test)]
mod tests {
    use cocomo_lib::{DirComparison, DirEntry, DirEntryStatus, SyncResult};

    use super::*;

    fn dir_session() -> SessionView {
        SessionView::new_dir_compare(
            PathBuf::from("/left"),
            PathBuf::from("/right"),
            false,
        )
    }

    fn comparison(entries: &[(&str, DirEntryStatus)]) -> DirComparison {
        DirComparison {
            entries: entries
                .iter()
                .map(|(name, status)| DirEntry {
                    name: (*name).to_string(),
                    status: *status,
                    left: None,
                    right: None,
                    center: None,
                    sub_entries: None,
                })
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn set_comparison_resets_selection() {
        let mut session = dir_session();
        session.ui.table_state.select(Some(7));
        session.set_comparison(Some(comparison(&[(
            "a",
            DirEntryStatus::Different,
        )])));
        assert!(session.dir().unwrap().comparison.is_some());
        assert_eq!(session.ui.table_state.selected(), Some(0));

        // The loading state resets the selection, too.
        session.ui.table_state.select(Some(3));
        session.set_comparison(None);
        assert!(session.dir().unwrap().comparison.is_none());
        assert_eq!(session.ui.table_state.selected(), Some(0));
    }

    #[test]
    fn content_guards_separate_dir_and_text_sessions() {
        // The `SessionContent` variant is the sole source of truth for the
        // session type: the accessors and `set_comparison` respect it, so
        // no dir-only state exists on a text session (and vice versa).
        let dir = dir_session();
        assert!(dir.dir().is_some());
        assert!(dir.text().is_none());
        assert!(dir.content.is_dir());

        let mut text = SessionView::new_text_compare(
            PathBuf::from("/left"),
            PathBuf::from("/right"),
            Vec::new(),
            Vec::new(),
            TextDiff::default(),
            Grammar::plain_text(),
        );
        assert!(text.text().is_some());
        assert!(text.dir().is_none());
        assert!(text.content.is_text());
        assert!(text.filtered_entries().is_empty());

        // `set_comparison` is a no-op on a text session.
        text.set_comparison(Some(comparison(&[(
            "a",
            DirEntryStatus::Different,
        )])));
        assert!(text.dir().is_none());
    }

    #[test]
    fn sync_stage_exposes_its_result_regardless_of_stage() {
        let planned = SyncStage::Planned(SyncResult::default());
        assert_eq!(planned.result().planned_count(), 0);
        let executed = SyncStage::Executed(SyncResult::default());
        assert!(executed.result().is_dry_run());
    }

    #[test]
    fn filtered_entries_applies_hide_same_and_name_filter() {
        let mut session = dir_session();
        session.set_comparison(Some(comparison(&[
            ("alpha", DirEntryStatus::Same),
            ("Beta", DirEntryStatus::Different),
            ("gamma", DirEntryStatus::LeftOnly),
        ])));
        assert_eq!(session.filtered_entries().len(), 3);

        session.dir_mut().unwrap().hide_same = true;
        assert_eq!(session.filtered_entries().len(), 2);

        // The name filter is case-insensitive and combines with hide_same.
        session.ui.active_filter = Some("b".into());
        let names: Vec<&str> = session
            .filtered_entries()
            .iter()
            .map(|e| e.name.as_str())
            .collect();
        assert_eq!(names, ["Beta"]);
    }

    #[test]
    fn row_count_tracks_filtered_entries() {
        let mut session = dir_session();
        assert_eq!(session.row_count(), 0);
        session.set_comparison(Some(comparison(&[
            ("a", DirEntryStatus::Same),
            ("b", DirEntryStatus::Different),
        ])));
        assert_eq!(session.row_count(), 2);
        session.dir_mut().unwrap().hide_same = true;
        assert_eq!(session.row_count(), 1);
    }

    #[test]
    fn tab_title_uses_file_names() {
        let session = SessionView::new_dir_compare(
            PathBuf::from("/a/b"),
            PathBuf::from("/x/y"),
            false,
        );
        assert_eq!(session.tab_title(), "b vs y");
    }

    #[test]
    fn add_session_activates_the_new_tab() {
        let mut app = AppState::new();
        app.add_session(dir_session());
        app.add_session(dir_session());
        assert_eq!(app.active_index(), 1);
        assert_eq!(app.sessions.len(), 2);
    }

    #[test]
    fn switch_tab_wraps_at_both_ends() {
        let mut app = AppState::new();
        for _ in 0..3 {
            app.add_session(dir_session());
        }
        assert_eq!(app.active_index(), 2);
        app.switch_tab(1);
        assert_eq!(app.active_index(), 0);
        app.switch_tab(-1);
        assert_eq!(app.active_index(), 2);
    }

    #[test]
    fn close_active_clamps_then_empties() {
        let mut app = AppState::new();
        let ids: Vec<usize> = (0..3)
            .map(|_| {
                let session = dir_session();
                let id = session.core.id;
                app.add_session(session);
                id
            })
            .collect();

        // The last tab is active; closing it activates the previous one.
        app.close_active();
        assert_eq!(app.active_index(), 1);
        assert_eq!(app.active().map(|s| s.core.id), Some(ids[1]));
        app.close_active();
        app.close_active();
        assert!(app.sessions.is_empty());
        assert!(app.active().is_none());

        // Closing with no sessions left is a no-op.
        app.close_active();
    }

    #[test]
    fn session_lookup_by_id_survives_tab_switch() {
        let mut app = AppState::new();
        app.add_session(dir_session());
        let id = app.active().unwrap().core.id;
        app.add_session(dir_session());
        app.switch_tab(-1);
        assert_eq!(app.active().map(|s| s.core.id), Some(id));
        assert!(app.session(id).is_some());
        assert!(app.session_mut(id).is_some());
        assert!(app.session(id + 1000).is_none());
    }
}
