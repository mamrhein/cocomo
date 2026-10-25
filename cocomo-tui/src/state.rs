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
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};

use cocomo_lib::{
    DirComparison, DirEntry, DirEntryStatus, Endpoint, Grammar, LineInfo,
    SyncOperation, SyncResult, TextDiff, TransferAction, TransferItem,
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
    /// The user is typing a search pattern (`/`); submitting jumps to the
    /// next matching row.
    Search { query: String },
    /// The user is typing a command-line command (`:`).
    Command { input: String },
    /// The transfer dialog (`c` copy, `d` delete) is open for the selected
    /// entry. Not a text-entry mode: the dialog's own input layer keeps the
    /// shell and the table from seeing keys, so `is_editing` stays `false`.
    Transfer { op: TransferOp },
}

/// Which transfer the [`SessionMode::Transfer`] dialog offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransferOp {
    /// Copy the entry to the other side.
    Copy,
    /// Delete the entry from one or both sides.
    Delete,
}

impl TransferOp {
    /// Title of the dialog for this operation.
    pub fn title(self) -> &'static str {
        match self {
            Self::Copy => "Copy",
            Self::Delete => "Delete",
        }
    }

    /// Return the options this operation offers for an entry present on the
    /// given sides, in display order and always ending with
    /// [`TransferChoice::Abort`].
    ///
    /// Options that cannot apply to the entry are omitted rather than shown
    /// dead: copying left → right needs a left side, and `both` only makes
    /// sense when both sides hold the entry.
    pub fn choices(
        self,
        has_left: bool,
        has_right: bool,
    ) -> Vec<TransferChoice> {
        let mut choices = Vec::new();
        if has_left {
            choices.push(TransferChoice::Left);
        }
        if has_right {
            choices.push(TransferChoice::Right);
        }
        if self == Self::Delete && has_left && has_right {
            choices.push(TransferChoice::Both);
        }
        choices.push(TransferChoice::Abort);
        choices
    }
}

/// One option of the transfer dialog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransferChoice {
    /// Copy left → right, respectively delete the left side.
    Left,
    /// Copy right → left, respectively delete the right side.
    Right,
    /// Delete both sides; only offered by [`TransferOp::Delete`].
    Both,
    /// Close the dialog without transferring.
    Abort,
}

impl TransferChoice {
    /// Return the dialog label of this option under `op`.
    pub fn label(self, op: TransferOp) -> &'static str {
        match (self, op) {
            (Self::Left, TransferOp::Copy) => "left \u{2192} right",
            (Self::Right, TransferOp::Copy) => "right \u{2192} left",
            (Self::Left, TransferOp::Delete) => "left",
            (Self::Right, TransferOp::Delete) => "right",
            (Self::Both, _) => "both",
            (Self::Abort, _) => "abort",
        }
    }

    /// Return the accelerator key shown next to the option.
    pub fn key(self) -> &'static str {
        match self {
            Self::Left => "l",
            Self::Right => "r",
            Self::Both => "b",
            Self::Abort => "Esc",
        }
    }
}

/// The entry a transfer acts on, resolved from the current selection.
///
/// Carries exactly what the dialog needs to offer its options (which sides
/// hold the entry) and what the executor needs to run the transfer, so the
/// two cannot disagree about the target.
#[derive(Clone, Debug, PartialEq)]
pub struct TransferTarget {
    /// Entry name; the executor addresses both sides by it.
    pub name: String,
    /// Path of the entry relative to the comparison root.
    pub rel_path: String,
    /// Whether the entry is a directory.
    pub is_dir: bool,
    /// The entry exists on the left side.
    pub has_left: bool,
    /// The entry exists on the right side.
    pub has_right: bool,
    /// Both endpoints address the same root, so one delete removes the entry
    /// for both sides.
    pub same_side: bool,
    /// Why no side option could be offered, shown dimmed in the dialog.
    pub note: Option<&'static str>,
}

impl TransferTarget {
    /// Build the [`TransferItem`]s for `choice`.
    ///
    /// Empty for [`TransferChoice::Abort`] and for options the entry does
    /// not support, so a stale accelerator can never transfer something the
    /// dialog never offered.
    pub fn items(
        &self,
        op: TransferOp,
        choice: TransferChoice,
    ) -> Vec<TransferItem> {
        let actions: Vec<TransferAction> = match (op, choice) {
            (TransferOp::Copy, TransferChoice::Left) if self.has_left => {
                vec![TransferAction::CopyRight]
            }
            (TransferOp::Copy, TransferChoice::Right) if self.has_right => {
                vec![TransferAction::CopyLeft]
            }
            (TransferOp::Delete, TransferChoice::Left) if self.has_left => {
                vec![TransferAction::DeleteLeft]
            }
            (TransferOp::Delete, TransferChoice::Right) if self.has_right => {
                vec![TransferAction::DeleteRight]
            }
            (TransferOp::Delete, TransferChoice::Both)
                if self.has_left && self.has_right =>
            {
                // On a shared root the two sides are the same entry, so a
                // second delete would fail on the file the first removed.
                let mut actions = vec![TransferAction::DeleteLeft];
                if !self.same_side {
                    actions.push(TransferAction::DeleteRight);
                }
                actions
            }
            _ => Vec::new(),
        };
        actions
            .into_iter()
            .map(|action| {
                TransferItem::new(
                    action,
                    self.name.clone(),
                    self.is_dir,
                    self.rel_path.clone(),
                )
            })
            .collect()
    }
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
            Self::Filter
                | Self::SaveSession { .. }
                | Self::LoadSession { .. }
                | Self::Search { .. }
                | Self::Command { .. }
        )
    }
}

/// Actions that a session's key handler can request from the App component.
#[derive(Clone, Debug, PartialEq)]
pub enum SessionAction {
    /// Open a subdirectory comparison in a new session.
    OpenDir { left: Endpoint, right: Endpoint },
    /// Open a file comparison (text diff) in a new session.
    OpenFile { left: Endpoint, right: Endpoint },
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
    /// Execute the transfers the dialog confirmed on the selected entry,
    /// then re-run the comparison so the view reflects the change.
    Transfer { items: Vec<TransferItem> },
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
    /// Left-side endpoint (directory or file).
    pub left: Endpoint,
    /// Right-side endpoint (directory or file).
    pub right: Endpoint,
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
    /// Bumped on every replacement of [`Self::comparison`].
    ///
    /// `DirComparison` does not implement `PartialEq`, so this counter is
    /// the change signal the dir view uses as its row-memoization key.
    pub comparison_revision: u64,
    /// Bumped on every sync outcome recorded through
    /// [`SessionView::set_sync`]. Keys the sync-preview row memoization.
    pub sync_revision: u64,
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
    /// the tab title from the two endpoints.
    fn new(left: Endpoint, right: Endpoint) -> Self {
        let name = Self::tab_title_for(&left, &right);
        Self {
            id: NEXT_SESSION_ID.fetch_add(1, Ordering::Relaxed),
            name,
            left,
            right,
        }
    }

    /// Generate a short tab title from the two endpoints' entry names.
    ///
    /// When both sides share the same name the pair carries no extra
    /// information, so the title is shortened to the single name.
    fn tab_title_for(left: &Endpoint, right: &Endpoint) -> String {
        let left_name = left
            .path()
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "…".to_string());
        let right_name = right
            .path()
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "…".to_string());
        if left_name == right_name {
            left_name
        } else {
            format!("{left_name} vs {right_name}")
        }
    }
}

impl SessionView {
    /// Create a new directory comparison session.
    pub fn new_dir_compare(
        left: Endpoint,
        right: Endpoint,
        compare_content: bool,
    ) -> Self {
        Self {
            core: SessionCore::new(left, right),
            content: SessionContent::Dir(DirView {
                comparison: None,
                comparison_revision: 0,
                sync_revision: 0,
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
        left: Endpoint,
        right: Endpoint,
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
            dir.comparison_revision += 1;
            self.ui.table_state.select(Some(0));
        }
    }

    /// Record the outcome of a sync run and reset the selection.
    ///
    /// Single owner of the `dir.sync` mutation: bumps the revision counter
    /// that keys the sync-preview row memoization and resets the table
    /// selection, mirroring [`SessionView::set_comparison`]. A no-op on a
    /// text session, which has no sync state.
    pub fn set_sync(&mut self, stage: SyncStage) {
        if let Some(dir) = self.dir_mut() {
            dir.sync = Some(stage);
            dir.sync_revision += 1;
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

    /// Resolve the transfer target for the current selection.
    ///
    /// A directory session targets the selected entry. A text session
    /// targets the compared files: the executor addresses both sides by one
    /// name, so a transfer is only possible when the two sides share a file
    /// name — otherwise the target carries no side options and a note
    /// explaining why. `None` when nothing is selected or the selected entry
    /// exists on neither side.
    pub fn transfer_target(&self) -> Option<TransferTarget> {
        let same_side = self.core.left.same_provider(&self.core.right)
            && self.core.left.path() == self.core.right.path();
        match &self.content {
            SessionContent::Dir(_) => {
                let entries = self.filtered_entries();
                let entry = self
                    .ui
                    .table_state
                    .selected()
                    .filter(|&i| i < entries.len())
                    .map(|i| entries[i])?;
                let info = entry.left.as_ref().or(entry.right.as_ref())?;
                Some(TransferTarget {
                    name: entry.name.clone(),
                    rel_path: info.path.clone(),
                    is_dir: info.is_dir,
                    has_left: entry.left.is_some(),
                    has_right: entry.right.is_some(),
                    same_side,
                    note: None,
                })
            }
            SessionContent::Text(_) => {
                let left_name = self
                    .core
                    .left
                    .path()
                    .file_name()?
                    .to_string_lossy()
                    .to_string();
                let right_name = self
                    .core
                    .right
                    .path()
                    .file_name()?
                    .to_string_lossy()
                    .to_string();
                let shared = left_name == right_name;
                Some(TransferTarget {
                    rel_path: left_name.clone(),
                    name: left_name,
                    is_dir: false,
                    has_left: shared,
                    has_right: shared,
                    same_side,
                    note: (!shared)
                        .then_some("the two sides have different file names"),
                })
            }
        }
    }

    /// Return the first row at or after `after + 1` whose displayed text
    /// contains `query` (case-insensitive), wrapping around at the end.
    ///
    /// Directory rows match on the entry name, text rows on either side's
    /// line content. This is the search model behind `/`; `None` when no
    /// row matches or the query is empty.
    pub fn find_row_matching(
        &self,
        query: &str,
        after: usize,
    ) -> Option<usize> {
        if query.is_empty() {
            return None;
        }
        let query = query.to_lowercase();
        let rows = self.row_count();
        if rows == 0 {
            return None;
        }
        let start = after.saturating_add(1);
        (0..rows)
            .map(|k| (start + k) % rows)
            .find(|&i| self.row_matches(i, &query))
    }

    /// Return whether the row at index `i` contains the lowercased `query`.
    fn row_matches(&self, i: usize, query: &str) -> bool {
        match &self.content {
            SessionContent::Dir(_) => self
                .filtered_entries()
                .get(i)
                .is_some_and(|e| e.name.to_lowercase().contains(query)),
            SessionContent::Text(text) => {
                [text.left_lines.get(i), text.right_lines.get(i)]
                    .into_iter()
                    .flatten()
                    .any(|line| line.content.to_lowercase().contains(query))
            }
        }
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
    /// Whether the bottom help bar is shown.
    ///
    /// Hidden by default so the main view area gets the row; toggled with
    /// `?` (see [`crate::keys::Cmd::ToggleHelp`]).
    pub show_help: bool,
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
            show_help: false,
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
    use cocomo_lib::{
        DirComparison, DirEntry, DirEntryStatus, EntryInfo, SyncResult,
    };

    use super::*;

    fn dir_session() -> SessionView {
        SessionView::new_dir_compare(
            Endpoint::local("/left"),
            Endpoint::local("/right"),
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
            Endpoint::local("/left"),
            Endpoint::local("/right"),
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
            Endpoint::local("/a/b"),
            Endpoint::local("/x/y"),
            false,
        );
        assert_eq!(session.tab_title(), "b vs y");
    }

    #[test]
    fn tab_title_shortens_when_names_are_equal() {
        let session = SessionView::new_dir_compare(
            Endpoint::local("/a/b"),
            Endpoint::local("/x/b"),
            false,
        );
        assert_eq!(session.tab_title(), "b");
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

    #[test]
    fn find_row_matching_wraps_and_ignores_case() {
        let mut session = dir_session();
        session.set_comparison(Some(comparison(&[
            ("Alpha", DirEntryStatus::Same),
            ("beta.txt", DirEntryStatus::Different),
            ("gamma", DirEntryStatus::Same),
        ])));

        // Case-insensitive substring match, starting strictly after the
        // given row.
        assert_eq!(session.find_row_matching("BETA", 0), Some(1));
        // Wraps around at the end back to the first match.
        assert_eq!(session.find_row_matching("alpha", 1), Some(0));
        // No match and the empty query yield `None`.
        assert_eq!(session.find_row_matching("delta", 0), None);
        assert_eq!(session.find_row_matching("", 0), None);
    }

    #[test]
    fn find_row_matching_searches_both_sides_of_a_text_diff() {
        let left = vec![
            LineInfo::new(1, "fn one()".into()),
            LineInfo::new(2, "fn two()".into()),
        ];
        let right = vec![
            LineInfo::new(1, "fn one()".into()),
            LineInfo::new(2, "let THREE = 3;".into()),
        ];
        let session = SessionView::new_text_compare(
            Endpoint::local("/left"),
            Endpoint::local("/right"),
            left,
            right,
            TextDiff::default(),
            Grammar::plain_text(),
        );

        // A match only on the right side is found, and only on the left.
        assert_eq!(session.find_row_matching("three", 0), Some(1));
        assert_eq!(session.find_row_matching("two", 0), Some(1));
        // From the matching row itself, the wrap-around still finds it (it
        // is the last candidate of the search).
        assert_eq!(session.find_row_matching("two", 1), Some(1));
        assert_eq!(session.find_row_matching("nine", 0), None);
    }

    fn entry_with_sides(
        name: &str,
        status: DirEntryStatus,
        left: bool,
        right: bool,
    ) -> DirEntry {
        let info = |side: &str| EntryInfo {
            name: name.to_string(),
            path: format!("/{side}/{name}"),
            ..Default::default()
        };
        DirEntry {
            name: name.to_string(),
            status,
            left: left.then(|| info("left")),
            right: right.then(|| info("right")),
            center: None,
            sub_entries: None,
        }
    }

    fn target(
        has_left: bool,
        has_right: bool,
        same_side: bool,
    ) -> TransferTarget {
        TransferTarget {
            name: "a.txt".into(),
            rel_path: "a.txt".into(),
            is_dir: false,
            has_left,
            has_right,
            same_side,
            note: None,
        }
    }

    #[test]
    fn choices_offer_only_the_options_the_sides_support() {
        assert_eq!(
            TransferOp::Copy.choices(true, true),
            vec![
                TransferChoice::Left,
                TransferChoice::Right,
                TransferChoice::Abort,
            ]
        );
        assert_eq!(
            TransferOp::Copy.choices(true, false),
            vec![TransferChoice::Left, TransferChoice::Abort]
        );
        assert_eq!(
            TransferOp::Copy.choices(false, true),
            vec![TransferChoice::Right, TransferChoice::Abort]
        );
        // Without any side the dialog only offers the way out.
        assert_eq!(
            TransferOp::Copy.choices(false, false),
            vec![TransferChoice::Abort]
        );

        // `both` is a delete-only option and needs both sides.
        assert_eq!(
            TransferOp::Delete.choices(true, true),
            vec![
                TransferChoice::Left,
                TransferChoice::Right,
                TransferChoice::Both,
                TransferChoice::Abort,
            ]
        );
        assert_eq!(
            TransferOp::Delete.choices(true, false),
            vec![TransferChoice::Left, TransferChoice::Abort]
        );
        assert_eq!(
            TransferOp::Delete.choices(false, true),
            vec![TransferChoice::Right, TransferChoice::Abort]
        );
    }

    #[test]
    fn items_map_each_choice_to_the_matching_actions() {
        let target = target(true, true, false);

        let actions = |op, choice| {
            target
                .items(op, choice)
                .iter()
                .map(|item| item.action)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            actions(TransferOp::Copy, TransferChoice::Left),
            vec![TransferAction::CopyRight]
        );
        assert_eq!(
            actions(TransferOp::Copy, TransferChoice::Right),
            vec![TransferAction::CopyLeft]
        );
        assert_eq!(
            actions(TransferOp::Delete, TransferChoice::Left),
            vec![TransferAction::DeleteLeft]
        );
        assert_eq!(
            actions(TransferOp::Delete, TransferChoice::Right),
            vec![TransferAction::DeleteRight]
        );
        assert_eq!(
            actions(TransferOp::Delete, TransferChoice::Both),
            vec![TransferAction::DeleteLeft, TransferAction::DeleteRight]
        );
        assert!(actions(TransferOp::Delete, TransferChoice::Abort).is_empty());

        // Every item carries the target's identity.
        let item = &target.items(TransferOp::Copy, TransferChoice::Left)[0];
        assert_eq!(item.name, "a.txt");
        assert_eq!(item.rel_path, "a.txt");
        assert!(!item.is_dir);
    }

    #[test]
    fn delete_both_on_a_shared_root_deletes_only_once() {
        // On a shared root the two sides are the same entry; a second
        // delete would fail on the file the first one removed.
        let target = target(true, true, true);
        assert_eq!(
            target.items(TransferOp::Delete, TransferChoice::Both),
            vec![TransferItem::new(
                TransferAction::DeleteLeft,
                "a.txt".into(),
                false,
                "a.txt".into(),
            )]
        );
    }

    #[test]
    fn items_are_empty_for_choices_the_target_does_not_offer() {
        // A stale accelerator must never transfer something the dialog
        // never offered.
        let left_only = target(true, false, false);
        assert!(
            left_only
                .items(TransferOp::Copy, TransferChoice::Right)
                .is_empty()
        );
        assert!(
            left_only
                .items(TransferOp::Delete, TransferChoice::Both)
                .is_empty()
        );

        let right_only = target(false, true, false);
        assert!(
            right_only
                .items(TransferOp::Copy, TransferChoice::Left)
                .is_empty()
        );
        assert!(
            right_only
                .items(TransferOp::Delete, TransferChoice::Both)
                .is_empty()
        );

        let neither = target(false, false, false);
        assert!(
            neither
                .items(TransferOp::Copy, TransferChoice::Left)
                .is_empty()
        );
        assert!(
            neither
                .items(TransferOp::Delete, TransferChoice::Right)
                .is_empty()
        );
    }

    #[test]
    fn transfer_target_resolves_the_selected_dir_entry() {
        let mut session = dir_session();
        session.set_comparison(Some(DirComparison {
            entries: vec![
                entry_with_sides(
                    "only-left",
                    DirEntryStatus::LeftOnly,
                    true,
                    false,
                ),
                entry_with_sides(
                    "both",
                    DirEntryStatus::Different,
                    true,
                    true,
                ),
            ],
            ..Default::default()
        }));

        session.ui.table_state.select(Some(0));
        let target = session.transfer_target().unwrap();
        assert_eq!(target.name, "only-left");
        assert!(target.has_left);
        assert!(!target.has_right);
        assert!(!target.same_side);
        assert_eq!(target.note, None);

        session.ui.table_state.select(Some(1));
        let target = session.transfer_target().unwrap();
        assert_eq!(target.name, "both");
        assert!(target.has_left && target.has_right);

        // Nothing selected, or a selection past the end, yields no target.
        session.ui.table_state.select(None);
        assert!(session.transfer_target().is_none());
        session.ui.table_state.select(Some(2));
        assert!(session.transfer_target().is_none());
    }

    #[test]
    fn transfer_target_requires_the_entry_to_exist_on_a_side() {
        // The shared `comparison()` helper builds entries without any side
        // metadata, so there is nothing to transfer.
        let mut session = dir_session();
        session.set_comparison(Some(comparison(&[(
            "a",
            DirEntryStatus::Different,
        )])));
        assert!(session.transfer_target().is_none());
    }

    #[test]
    fn transfer_target_detects_a_shared_root() {
        let mut session = SessionView::new_dir_compare(
            Endpoint::local("/same"),
            Endpoint::local("/same"),
            false,
        );
        session.set_comparison(Some(DirComparison {
            entries: vec![entry_with_sides(
                "a",
                DirEntryStatus::Different,
                true,
                true,
            )],
            ..Default::default()
        }));
        session.ui.table_state.select(Some(0));
        assert!(session.transfer_target().unwrap().same_side);
    }

    #[test]
    fn text_transfer_target_gates_on_matching_file_names() {
        let text = SessionView::new_text_compare(
            Endpoint::local("/left/a.txt"),
            Endpoint::local("/right/a.txt"),
            Vec::new(),
            Vec::new(),
            TextDiff::default(),
            Grammar::plain_text(),
        );
        let target = text.transfer_target().unwrap();
        assert_eq!(target.name, "a.txt");
        assert!(target.has_left && target.has_right);
        assert_eq!(target.note, None);

        // The executor addresses both sides by one name, so differing file
        // names disable all side options and explain why.
        let mismatched = SessionView::new_text_compare(
            Endpoint::local("/left/a.txt"),
            Endpoint::local("/right/b.txt"),
            Vec::new(),
            Vec::new(),
            TextDiff::default(),
            Grammar::plain_text(),
        );
        let target = mismatched.transfer_target().unwrap();
        assert!(!target.has_left && !target.has_right);
        assert!(target.note.is_some());
    }
}
