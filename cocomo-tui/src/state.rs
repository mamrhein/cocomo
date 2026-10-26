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
    collections::HashSet,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};

use cocomo_lib::{
    DirComparison, DirEntry, Endpoint, Grammar, LineInfo, SyncOperation,
    SyncResult, TextDiff, TransferAction, TransferItem,
    filter::DisplayFilters,
    report::{ReportConfig, ReportFormat},
};
use ratatui_kit::{Atom, components::table::TableState};
use regex::RegexBuilder;

/// The current input mode of a session.
///
/// Each text-entry mode owns its input buffers, so a buffer cannot leak
/// from one mode into another: leaving a mode drops its state with it.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum SessionMode {
    #[default]
    Normal,
    /// The filter panel modal (`f`) is open, holding its draft in a box
    /// so the enum stays small.
    FilterPanel { draft: Box<FilterDraft> },
    /// The user is typing a session name to save.
    SaveSession { name: String },
    /// The session browser modal (`O`) is open. `saved_sessions` is
    /// filled asynchronously by the `ListSessions` action.
    SessionBrowser { saved_sessions: Vec<String> },
    /// The settings modal (`,`) is open.
    Settings,
    /// The report modal (`R`) is open, holding its draft in a box.
    Report { draft: Box<ReportDraft> },
    /// The user is typing a search pattern (`/`); submitting jumps to the
    /// next matching row.
    Search { query: String },
    /// The user is typing a command-line command (`:`).
    Command { input: String },
    /// The transfer dialog (`c` copy, `d` delete) is open for the selected
    /// entry. Not a text-entry mode: the dialog's own input layer keeps the
    /// shell and the table from seeing keys, so `is_editing` stays `false`.
    Transfer { op: TransferOp },
    /// The sync confirmation modal is open for the planned sync (`S`). Like
    /// [`SessionMode::Transfer`], the modal's exclusive input layer owns
    /// every key while it is open, so this is not a text-entry mode either.
    SyncConfirm,
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

    /// The dialog's fixed option list, in display order.
    ///
    /// All four options are always listed; [`TransferChoice::is_enabled`]
    /// decides which ones the selected entry actually activates, so the
    /// user sees the full key map instead of a list that shifts shape per
    /// entry.
    pub fn choices(self) -> [TransferChoice; 4] {
        [
            TransferChoice::Left,
            TransferChoice::Right,
            TransferChoice::Both,
            TransferChoice::Abort,
        ]
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

    /// Return whether an entry present on the given sides activates this
    /// option under `op`.
    ///
    /// `l` needs a left side, `r` a right side, and `b` both sides; `abort`
    /// is always active. `both` stays a delete-only option: copying in both
    /// directions is order-dependent and nets out to a no-op in a 2-way
    /// compare.
    pub fn is_enabled(
        self,
        op: TransferOp,
        has_left: bool,
        has_right: bool,
    ) -> bool {
        match self {
            Self::Left => has_left,
            Self::Right => has_right,
            Self::Both => op == TransferOp::Delete && has_left && has_right,
            Self::Abort => true,
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
    /// not quit). The modal panels (`FilterPanel`, `SessionBrowser`,
    /// `Settings`, `Report`) are not text-entry modes: like
    /// [`SessionMode::Transfer`] they own an exclusive input layer that
    /// keeps the shell and the table from seeing keys.
    pub fn is_editing(&self) -> bool {
        matches!(
            self,
            Self::SaveSession { .. }
                | Self::Search { .. }
                | Self::Command { .. }
        )
    }
}

/// A compiled, case-insensitive regex used as the directory view's name
/// filter.
///
/// The pattern text is kept alongside the compiled form so the filter can
/// be displayed, edited, persisted, and compared cheaply (`PartialEq`
/// compares the text only, since `Regex` is not comparable).
#[derive(Clone, Debug)]
pub struct NamePattern {
    /// The pattern text as the user entered it.
    pub pattern: String,
    /// The case-insensitive compiled form.
    regex: regex::Regex,
}

impl NamePattern {
    /// Compile `pattern` into a case-insensitive name filter.
    pub fn new(pattern: impl Into<String>) -> Result<Self, regex::Error> {
        let pattern = pattern.into();
        let regex =
            RegexBuilder::new(&pattern).case_insensitive(true).build()?;
        Ok(Self { pattern, regex })
    }

    /// Returns `true` when `name` matches the pattern.
    pub fn is_match(&self, name: &str) -> bool {
        self.regex.is_match(name)
    }
}

impl PartialEq for NamePattern {
    fn eq(&self, other: &Self) -> bool {
        self.pattern == other.pattern
    }
}

/// Which section of the filter panel owns the keys.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FilterFocus {
    /// The regex pattern field.
    #[default]
    Pattern,
    /// The name-filter enable toggle.
    NameToggle,
    /// The status checkboxes.
    Statuses,
}

/// Draft state of the filter panel modal (`f`).
///
/// All fields are primitives so the draft can be `PartialEq` (neither a
/// compiled `Regex` nor [`DisplayFilters`] is comparable). The panel
/// applies the draft to the session on `Enter`; an invalid regex keeps the
/// panel open with [`Self::error`] set.
#[derive(Clone, Debug, PartialEq)]
pub struct FilterDraft {
    /// Name pattern text.
    pub pattern: String,
    /// Cursor position within [`Self::pattern`] (character index).
    pub cursor: usize,
    /// Whether the name pattern filters entries at all.
    pub name_filter_on: bool,
    /// Show identical entries.
    pub show_same: bool,
    /// Show differing entries.
    pub show_different: bool,
    /// Show orphan entries.
    pub show_orphans: bool,
    /// Which section owns the keys.
    pub focus: FilterFocus,
    /// Validation message shown in the panel (e.g. an invalid regex).
    pub error: Option<String>,
}

impl Default for FilterDraft {
    fn default() -> Self {
        Self {
            pattern: String::new(),
            cursor: 0,
            name_filter_on: true,
            show_same: true,
            show_different: true,
            show_orphans: true,
            focus: FilterFocus::default(),
            error: None,
        }
    }
}

/// Which section of the report modal owns the keys.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ReportFocus {
    /// The format list.
    #[default]
    Format,
    /// The destination path field.
    Destination,
    /// The inclusion checkboxes.
    Options,
}

/// Draft state of the report modal (`R`).
///
/// Like [`FilterDraft`] the fields are primitives so the draft stays
/// `PartialEq`; the [`ReportConfig`] is built from it only when the report
/// is generated.
#[derive(Clone, Debug, PartialEq)]
pub struct ReportDraft {
    /// Output format.
    pub format: ReportFormat,
    /// Output file path text.
    pub destination: String,
    /// Cursor position within [`Self::destination`] (character index).
    pub cursor: usize,
    /// Include identical entries.
    pub include_same: bool,
    /// Include differing entries.
    pub include_different: bool,
    /// Include orphan entries.
    pub include_orphans: bool,
    /// Recurse into subdirectory entries.
    pub include_subdirectories: bool,
    /// Include detailed file metadata.
    pub include_file_details: bool,
    /// Which section owns the keys.
    pub focus: ReportFocus,
    /// Validation message shown in the modal (e.g. a missing path).
    pub error: Option<String>,
}

impl Default for ReportDraft {
    fn default() -> Self {
        Self {
            format: ReportFormat::default(),
            destination: String::new(),
            cursor: 0,
            include_same: false,
            include_different: true,
            include_orphans: true,
            include_subdirectories: true,
            include_file_details: true,
            focus: ReportFocus::default(),
            error: None,
        }
    }
}

impl ReportDraft {
    /// Build the [`ReportConfig`] the draft's checkboxes describe.
    pub fn config(&self) -> ReportConfig {
        ReportConfig {
            include_same: self.include_same,
            include_different: self.include_different,
            include_orphans: self.include_orphans,
            include_subdirectories: self.include_subdirectories,
            include_file_details: self.include_file_details,
        }
    }
}

/// A toggleable entry of the settings modal (`,`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingItem {
    /// Toggle content comparison (mirrors `C`).
    CompareContent,
    /// Toggle hiding identical entries (mirrors `F`).
    HideSame,
    /// Cycle the sync operation forward (mirrors `m`).
    SyncOperation,
}

/// A [`ReportConfig`] with field-wise equality.
///
/// `ReportConfig` itself does not implement `PartialEq`, but
/// [`SessionAction`] needs one so tests can assert on dispatched actions;
/// the newtype compares the inclusion flags directly.
#[derive(Clone, Debug)]
pub struct ReportSelection(pub ReportConfig);

impl PartialEq for ReportSelection {
    fn eq(&self, other: &Self) -> bool {
        let (a, b) = (&self.0, &other.0);
        a.include_same == b.include_same
            && a.include_different == b.include_different
            && a.include_orphans == b.include_orphans
            && a.include_subdirectories == b.include_subdirectories
            && a.include_file_details == b.include_file_details
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
    /// Plan a sync (dry-run), display the preview, then ask for approval
    /// of the execution in the confirmation modal.
    PlanSync { operation: SyncOperation },
    /// Execute the planned sync; dispatched when the confirmation modal is
    /// approved.
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
    /// Generate a comparison report and write it to `destination`; the
    /// report modal dispatches this on `Enter`.
    GenerateReport {
        format: ReportFormat,
        config: ReportSelection,
        destination: PathBuf,
    },
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
    /// Relative paths (`/`-separated, comparison-root based) of the
    /// directories the user has expanded in the tree view.
    ///
    /// Keyed by path rather than by entry identity so the set survives a
    /// reload: directories that still exist in the fresh comparison stay
    /// open, vanished ones simply never match a row.
    pub expanded: HashSet<String>,
    /// Bumped on every expand/collapse. Together with
    /// [`Self::comparison_revision`] keys the dir-row memoization.
    pub tree_revision: u64,
    /// Compare file contents (vs. meta data only).
    pub compare_content: bool,
    /// Which entry statuses the tree displays (the `F` toggle flips
    /// `show_same`).
    pub display: DisplayFilters,
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

/// One visible row of the directory view's expandable tree.
///
/// The rows are the filtered comparison flattened depth-first: a directory
/// row is followed by its children exactly while the directory is expanded,
/// so the table selection indexes into this list, not into the entry tree.
pub struct DirRow<'a> {
    /// The comparison entry rendered by this row.
    pub entry: &'a DirEntry,
    /// The entry's path relative to the comparison root, `/`-separated.
    /// This is the key the expand/collapse state is tracked by.
    pub rel_path: String,
    /// Nesting level below the comparison root; top-level rows are `0`.
    pub depth: usize,
    /// Whether the entry is a directory and can therefore be expanded.
    pub is_dir: bool,
    /// Whether the directory row is currently expanded.
    pub expanded: bool,
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
    /// Anchor row of a mouse drag selection.
    ///
    /// While set, the selection covers the inclusive range between the
    /// anchor and the table cursor. Keyboard navigation always produces a
    /// single-row selection and clears it (see [`SessionView::set_cursor`]).
    pub selection_anchor: Option<usize>,
    /// Current input mode (owns the save-name buffer and the panel drafts).
    pub mode: SessionMode,
    /// Active name filter. `None` means no filter.
    pub name_filter: Option<NamePattern>,
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
                expanded: HashSet::new(),
                tree_revision: 0,
                compare_content,
                display: DisplayFilters::all(),
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
            self.set_cursor(Some(0));
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
            self.set_cursor(Some(0));
        }
    }

    /// Move the table cursor to `index` and drop the drag-selection anchor.
    ///
    /// Keyboard navigation and programmatic jumps always produce a
    /// single-row selection, so they must not leave a stale range behind.
    /// Mouse click/drag selection goes through the controller
    /// (`select_row`/`extend_selection`) instead, which manages the anchor.
    pub fn set_cursor(&mut self, index: Option<usize>) {
        self.ui.selection_anchor = None;
        self.ui.table_state.select(index);
    }

    /// The inclusive row range covered by a drag selection.
    ///
    /// `None` while there is no drag anchor — then only the selected row
    /// is highlighted (the table widget already marks it).
    pub fn selection_range(&self) -> Option<(usize, usize)> {
        let selected = self.ui.table_state.selected()?;
        let anchor = self.ui.selection_anchor?;
        Some((anchor.min(selected), anchor.max(selected)))
    }

    /// Return the tab title for this session.
    pub fn tab_title(&self) -> &str {
        &self.core.name
    }

    /// Number of table rows this session currently displays.
    ///
    /// Directory sessions show the visible tree rows; text sessions show one
    /// row per line of the merged side-by-side view. Navigation must clamp
    /// against this count, not against [`SessionView::visible_rows`],
    /// which is empty for text sessions.
    pub fn row_count(&self) -> usize {
        match &self.content {
            SessionContent::Dir(_) => self.visible_rows().len(),
            SessionContent::Text(text) => {
                std::cmp::max(text.left_lines.len(), text.right_lines.len())
            }
        }
    }

    /// Return the visible tree rows: the filtered entries flattened
    /// depth-first, with a directory's children included while the
    /// directory is expanded.
    ///
    /// Empty for text sessions, which have no directory entries.
    pub fn visible_rows(&self) -> Vec<DirRow<'_>> {
        let Some(dir) = self.dir() else {
            return Vec::new();
        };
        let Some(comparison) = dir.comparison.as_ref() else {
            return Vec::new();
        };
        let mut rows = Vec::new();
        self.collect_rows(&comparison.entries, "", 0, &mut rows);
        rows
    }

    /// Recursively append the visible rows of one comparison level.
    ///
    /// The filters apply at every depth; a directory hidden by the name
    /// filter hides its whole subtree with it.
    fn collect_rows<'a>(
        &self,
        entries: &'a [DirEntry],
        prefix: &str,
        depth: usize,
        rows: &mut Vec<DirRow<'a>>,
    ) {
        let Some(dir) = self.dir() else {
            return;
        };
        for entry in entries {
            if !self.entry_visible(dir, entry) {
                continue;
            }
            let rel_path = if prefix.is_empty() {
                entry.name.clone()
            } else {
                format!("{prefix}/{}", entry.name)
            };
            let is_dir = entry.left.as_ref().is_some_and(|l| l.is_dir)
                || entry.right.as_ref().is_some_and(|r| r.is_dir);
            let expanded = is_dir && dir.expanded.contains(&rel_path);
            rows.push(DirRow {
                entry,
                rel_path: rel_path.clone(),
                depth,
                is_dir,
                expanded,
            });
            if expanded && let Some(sub) = entry.sub_entries.as_deref() {
                self.collect_rows(&sub.entries, &rel_path, depth + 1, rows);
            }
        }
    }

    /// Whether an entry passes the display and name filters.
    fn entry_visible(&self, dir: &DirView, entry: &DirEntry) -> bool {
        // Hide entries whose status is switched off in the display filters.
        if !dir.display.should_show(&entry.status) {
            return false;
        }

        // Apply the name filter (a case-insensitive regex).
        if self
            .ui
            .name_filter
            .as_ref()
            .is_some_and(|f| !f.is_match(&entry.name))
        {
            return false;
        }

        true
    }

    /// The row under the table selection, `None` when nothing is selected
    /// or the selection is out of range.
    pub fn selected_row(&self) -> Option<DirRow<'_>> {
        let selected = self.ui.table_state.selected()?;
        self.visible_rows().into_iter().nth(selected)
    }

    /// Return the index of the row with the given relative path.
    pub fn row_index_of(&self, rel_path: &str) -> Option<usize> {
        self.visible_rows()
            .iter()
            .position(|row| row.rel_path == rel_path)
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
        let same_side = self.same_side();
        match &self.content {
            SessionContent::Dir(_) => {
                let row = self.selected_row()?;
                Self::dir_target(&row, same_side)
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

    /// Resolve the transfer targets for the current selection.
    ///
    /// A drag-selected range transfers every row in it that exists on at
    /// least one side; without a drag anchor this is just the selected row
    /// (matching [`SessionView::transfer_target`]). Text sessions always
    /// target the compared files.
    pub fn transfer_targets(&self) -> Vec<TransferTarget> {
        let same_side = self.same_side();
        match &self.content {
            SessionContent::Dir(_) => {
                let selected = self.ui.table_state.selected();
                let Some((lo, hi)) = self
                    .selection_range()
                    .or_else(|| selected.map(|i| (i, i)))
                else {
                    return Vec::new();
                };
                let rows = self.visible_rows();
                if rows.is_empty() || lo >= rows.len() {
                    return Vec::new();
                }
                let hi = hi.min(rows.len() - 1);
                rows[lo..=hi]
                    .iter()
                    .filter_map(|row| Self::dir_target(row, same_side))
                    .collect()
            }
            SessionContent::Text(_) => {
                self.transfer_target().into_iter().collect()
            }
        }
    }

    /// Whether both endpoints address the same root, so one delete removes
    /// an entry for both sides.
    fn same_side(&self) -> bool {
        self.core.left.same_provider(&self.core.right)
            && self.core.left.path() == self.core.right.path()
    }

    /// Build the transfer target for one visible directory row, `None`
    /// when the entry exists on neither side.
    fn dir_target(
        row: &DirRow<'_>,
        same_side: bool,
    ) -> Option<TransferTarget> {
        let entry = row.entry;
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
                .visible_rows()
                .get(i)
                .is_some_and(|r| r.entry.name.to_lowercase().contains(query)),
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

    /// Activate the tab at `index`, ignoring out-of-range indices.
    ///
    /// The tab bar's click handler addresses tabs by absolute position,
    /// unlike [`AppState::switch_tab`]'s relative delta.
    pub fn set_active_tab(&mut self, index: usize) {
        if index < self.sessions.len() {
            self.active_tab = index;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

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

    /// A directory entry present on both sides, with an optional
    /// sub-comparison attached as its tree children.
    fn dir_entry(
        name: &str,
        status: DirEntryStatus,
        sub: Option<DirComparison>,
    ) -> DirEntry {
        let info = || EntryInfo {
            name: name.to_string(),
            path: name.to_string(),
            is_dir: true,
            ..Default::default()
        };
        DirEntry {
            name: name.to_string(),
            status,
            left: Some(info()),
            right: Some(info()),
            center: None,
            sub_entries: sub.map(Arc::new),
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
        assert!(text.visible_rows().is_empty());

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
    fn sync_confirm_is_not_an_editing_mode() {
        // The confirmation modal owns the keys through its own exclusive
        // input layer, not through a text-entry mode guard, so `is_editing`
        // must stay `false` (mirroring `Transfer`).
        assert!(!SessionMode::SyncConfirm.is_editing());
    }

    #[test]
    fn visible_rows_applies_display_and_name_filters() {
        let mut session = dir_session();
        session.set_comparison(Some(comparison(&[
            ("alpha", DirEntryStatus::Same),
            ("Beta", DirEntryStatus::Different),
            ("gamma", DirEntryStatus::LeftOnly),
        ])));
        assert_eq!(session.visible_rows().len(), 3);

        session.dir_mut().unwrap().display.show_same = false;
        assert_eq!(session.visible_rows().len(), 2);

        // The name filter is a case-insensitive regex and combines with
        // the display filters.
        session.ui.name_filter = Some(NamePattern::new("^b").unwrap());
        let names: Vec<&str> = session
            .visible_rows()
            .iter()
            .map(|r| r.entry.name.as_str())
            .collect();
        assert_eq!(names, ["Beta"]);
    }

    #[test]
    fn visible_rows_flatten_expanded_directories() {
        let mut session = dir_session();
        let sub = comparison(&[("child.txt", DirEntryStatus::Same)]);
        session.set_comparison(Some(DirComparison {
            entries: vec![dir_entry(
                "sub",
                DirEntryStatus::Different,
                Some(sub),
            )],
            ..Default::default()
        }));

        // Collapsed: only the directory row is visible.
        assert_eq!(session.row_count(), 1);
        let rows = session.visible_rows();
        assert!(rows[0].is_dir);
        assert!(!rows[0].expanded);

        // Expanding reveals the children one level deeper, with the
        // directory row reporting its expanded state.
        session.dir_mut().unwrap().expanded.insert("sub".into());
        let rows = session.visible_rows();
        assert_eq!(rows.len(), 2);
        assert!(rows[0].expanded);
        assert_eq!(rows[1].entry.name, "child.txt");
        assert_eq!(rows[1].depth, 1);
        assert_eq!(rows[1].rel_path, "sub/child.txt");
        assert!(!rows[1].is_dir);
    }

    #[test]
    fn filters_apply_at_every_depth() {
        let mut session = dir_session();
        let sub = comparison(&[
            ("keep.txt", DirEntryStatus::Different),
            ("drop.txt", DirEntryStatus::Same),
        ]);
        session.set_comparison(Some(DirComparison {
            entries: vec![dir_entry(
                "sub",
                DirEntryStatus::Different,
                Some(sub),
            )],
            ..Default::default()
        }));
        session.dir_mut().unwrap().expanded.insert("sub".into());
        session.dir_mut().unwrap().display.show_same = false;

        let names: Vec<&str> = session
            .visible_rows()
            .iter()
            .map(|r| r.entry.name.as_str())
            .collect();
        assert_eq!(names, ["sub", "keep.txt"]);

        // A directory hidden by the name filter hides its whole subtree.
        session.ui.name_filter = Some(NamePattern::new("keep").unwrap());
        assert!(session.visible_rows().is_empty());
    }

    #[test]
    fn row_index_of_finds_the_row_for_a_path() {
        let mut session = dir_session();
        let sub = comparison(&[("child.txt", DirEntryStatus::Same)]);
        session.set_comparison(Some(DirComparison {
            entries: vec![dir_entry(
                "sub",
                DirEntryStatus::Different,
                Some(sub),
            )],
            ..Default::default()
        }));
        assert_eq!(session.row_index_of("sub"), Some(0));
        // The child only has a row while the directory is expanded.
        assert_eq!(session.row_index_of("sub/child.txt"), None);
        session.dir_mut().unwrap().expanded.insert("sub".into());
        assert_eq!(session.row_index_of("sub/child.txt"), Some(1));
        assert_eq!(session.row_index_of("missing"), None);
    }

    #[test]
    fn row_count_tracks_visible_rows() {
        let mut session = dir_session();
        assert_eq!(session.row_count(), 0);
        session.set_comparison(Some(comparison(&[
            ("a", DirEntryStatus::Same),
            ("b", DirEntryStatus::Different),
        ])));
        assert_eq!(session.row_count(), 2);
        session.dir_mut().unwrap().display.show_same = false;
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
    fn choices_always_list_all_four_options() {
        let expected = [
            TransferChoice::Left,
            TransferChoice::Right,
            TransferChoice::Both,
            TransferChoice::Abort,
        ];
        assert_eq!(TransferOp::Copy.choices(), expected);
        assert_eq!(TransferOp::Delete.choices(), expected);
    }

    #[test]
    fn is_enabled_follows_the_sides_holding_the_entry() {
        use TransferChoice as C;
        use TransferOp::{Copy, Delete};

        // `l` activates with a left side, `r` with a right side, `b` with
        // both, and `abort` always.
        assert!(C::Left.is_enabled(Copy, true, false));
        assert!(!C::Left.is_enabled(Copy, false, true));
        assert!(C::Right.is_enabled(Copy, false, true));
        assert!(!C::Right.is_enabled(Copy, true, false));
        assert!(C::Both.is_enabled(Delete, true, true));
        assert!(!C::Both.is_enabled(Delete, true, false));
        assert!(!C::Both.is_enabled(Delete, false, true));
        assert!(C::Abort.is_enabled(Copy, false, false));

        // Copying in both directions is a no-op, so `b` never activates
        // for copy, even with both sides present.
        assert!(!C::Both.is_enabled(Copy, true, true));
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
    fn set_cursor_clears_the_selection_anchor() {
        let mut session = dir_session();
        session.ui.table_state.select(Some(2));
        session.ui.selection_anchor = Some(0);
        assert_eq!(session.selection_range(), Some((0, 2)));

        session.set_cursor(Some(5));
        assert_eq!(session.ui.table_state.selected(), Some(5));
        assert_eq!(session.ui.selection_anchor, None);
        assert_eq!(session.selection_range(), None);
    }

    #[test]
    fn selection_range_spans_anchor_and_cursor() {
        let mut session = dir_session();
        // Without an anchor the range is `None`, even with a selection.
        session.ui.table_state.select(Some(3));
        assert_eq!(session.selection_range(), None);

        // The range is inclusive and order-independent.
        session.ui.selection_anchor = Some(5);
        assert_eq!(session.selection_range(), Some((3, 5)));
        session.ui.table_state.select(Some(1));
        assert_eq!(session.selection_range(), Some((1, 5)));
    }

    #[test]
    fn resets_clear_the_selection_anchor() {
        let mut session = dir_session();
        session.ui.table_state.select(Some(2));
        session.ui.selection_anchor = Some(0);
        session.set_comparison(Some(comparison(&[(
            "a",
            DirEntryStatus::Different,
        )])));
        assert_eq!(session.ui.selection_anchor, None);

        session.ui.selection_anchor = Some(1);
        session.set_sync(SyncStage::Planned(SyncResult::default()));
        assert_eq!(session.ui.selection_anchor, None);
    }

    #[test]
    fn transfer_targets_cover_the_drag_range() {
        let mut session = dir_session();
        session.set_comparison(Some(DirComparison {
            entries: vec![
                entry_with_sides("a", DirEntryStatus::LeftOnly, true, false),
                // An entry on neither side (the `comparison()` shape) is
                // skipped by the range expansion.
                DirEntry {
                    name: "ghost".into(),
                    status: DirEntryStatus::Different,
                    left: None,
                    right: None,
                    center: None,
                    sub_entries: None,
                },
                entry_with_sides("c", DirEntryStatus::Different, true, true),
            ],
            ..Default::default()
        }));

        // Without an anchor the range is just the selected row.
        session.ui.table_state.select(Some(0));
        let names: Vec<String> = session
            .transfer_targets()
            .into_iter()
            .map(|t| t.name)
            .collect();
        assert_eq!(names, vec!["a"]);

        // With an anchor, every row in the range that exists on a side
        // becomes a target; the side-less row is skipped.
        session.ui.selection_anchor = Some(0);
        session.ui.table_state.select(Some(2));
        let names: Vec<String> = session
            .transfer_targets()
            .into_iter()
            .map(|t| t.name)
            .collect();
        assert_eq!(names, vec!["a", "c"]);
    }

    #[test]
    fn transfer_targets_clamp_to_the_visible_rows() {
        let mut session = dir_session();
        session.set_comparison(Some(DirComparison {
            entries: vec![entry_with_sides(
                "a",
                DirEntryStatus::Different,
                true,
                true,
            )],
            ..Default::default()
        }));
        // A stale anchor/cursor beyond the last row (e.g. after filtering)
        // must not panic and still yields the reachable rows.
        session.ui.selection_anchor = Some(0);
        session.ui.table_state.select(Some(9));
        let names: Vec<String> = session
            .transfer_targets()
            .into_iter()
            .map(|t| t.name)
            .collect();
        assert_eq!(names, vec!["a"]);

        // A selection entirely past the end yields no targets.
        session.ui.selection_anchor = None;
        session.ui.table_state.select(Some(9));
        assert!(session.transfer_targets().is_empty());
    }

    #[test]
    fn set_active_tab_ignores_out_of_range_indices() {
        let mut app = AppState::new();
        app.add_session(dir_session());
        app.add_session(dir_session());
        assert_eq!(app.active_index(), 1);

        app.set_active_tab(0);
        assert_eq!(app.active_index(), 0);

        app.set_active_tab(2);
        assert_eq!(app.active_index(), 0);
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
