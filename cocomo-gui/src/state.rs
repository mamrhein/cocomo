// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Application state for the GUI.
//!
//! The [`AppState`] entity holds the left and right comparison endpoints
//! and the results of the comparison. It provides methods to load and
//! compare directories asynchronously, and the UI observes this entity
//! for updates.

use std::{cmp::Ordering, sync::Arc, time::Instant};

use cocomo_lib::{
    CancellationToken, CompareConfig, ContentCache, DirComparison, DirEntry,
    DirEntryStatus, Endpoint, FsPair, ProviderRef, SessionConfig,
    SessionSettings, SessionType, TransferAction, TransferItem,
    compare_content_pair_node, compare_directories_node,
    compare_directories_pair_node, execute_transfers,
};
use gpui::{App, Context, FocusHandle, SharedString};

use crate::log::{self, LogLevel};

// ---------------------------------------------------------------------------
// Sorting
// ---------------------------------------------------------------------------

/// Column a table sort is keyed on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortKey {
    /// Comparison status (groups identical, different, and orphan entries).
    Status,
    /// Entry name, case-insensitive.
    Name,
    /// Left-side size; entries missing on the left sort last.
    SizeLeft,
    /// Right-side size; entries missing on the right sort last.
    SizeRight,
    /// Left-side modification time; entries missing on the left sort last.
    DateLeft,
    /// Right-side modification time; entries missing on the right sort last.
    DateRight,
}

// ---------------------------------------------------------------------------
// Display filters
// ---------------------------------------------------------------------------

/// Coarse category an entry belongs to for display filtering.
///
/// The three categories partition every [`DirEntryStatus`], so the filter
/// toggles and the status-bar summary counts always agree on where an
/// entry is counted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusCategory {
    /// Entry matches on both sides.
    Same,
    /// Entry exists on both sides but differs in content or state.
    Different,
    /// Entry exists on only one side.
    Orphan,
}

/// Map an entry status to its display-filter category.
pub fn status_category(status: &DirEntryStatus) -> StatusCategory {
    match status {
        DirEntryStatus::Same | DirEntryStatus::SameBinary => {
            StatusCategory::Same
        }
        DirEntryStatus::Similar
        | DirEntryStatus::Different
        | DirEntryStatus::Mergeable
        | DirEntryStatus::Conflict => StatusCategory::Different,
        DirEntryStatus::LeftOnly
        | DirEntryStatus::RightOnly
        | DirEntryStatus::CenterOnly => StatusCategory::Orphan,
    }
}

/// Exclusive display mode driven by the shared toolbar's toggle group.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FilterMode {
    /// Every category is visible.
    All,
    /// Only entries that differ in any way (different or orphan).
    Diffs,
    /// Only entries that match on both sides.
    Same,
    /// A combination the toggle group cannot express; no button reads as
    /// selected.
    Custom,
}

/// Which entry categories the table displays.
///
/// Filters are display-only: the underlying comparison and the summary
/// counts of hidden categories stay intact, so toggling a filter is
/// always reversible without re-comparing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DisplayFilters {
    /// Show entries that match on both sides.
    pub show_same: bool,
    /// Show entries that exist on both sides but differ.
    pub show_different: bool,
    /// Show entries that exist on only one side.
    pub show_orphans: bool,
}

impl Default for DisplayFilters {
    fn default() -> Self {
        Self {
            show_same: true,
            show_different: true,
            show_orphans: true,
        }
    }
}

impl DisplayFilters {
    /// Whether every category is visible, i.e. no filtering is active.
    pub fn all_visible(&self) -> bool {
        self.show_same && self.show_different && self.show_orphans
    }

    /// Whether entries of `category` appear in the table.
    pub fn shows(&self, category: StatusCategory) -> bool {
        match category {
            StatusCategory::Same => self.show_same,
            StatusCategory::Different => self.show_different,
            StatusCategory::Orphan => self.show_orphans,
        }
    }
}

// ---------------------------------------------------------------------------
// Sorting
// ---------------------------------------------------------------------------

/// Compute the display order (indices into `entries`) for a sort.
///
/// Entries hidden by `filters` are excluded outright. Missing side values
/// always sort last regardless of direction, so an orphan never jumps to
/// the top of a size or date sort. Ties break on name, which keeps the
/// order stable and predictable across re-sorts.
fn compute_display_order(
    entries: &[DirEntry],
    key: SortKey,
    descending: bool,
    filters: &DisplayFilters,
) -> Vec<usize> {
    let mut order: Vec<usize> = (0..entries.len())
        .filter(|&i| filters.shows(status_category(&entries[i].status)))
        .collect();
    order.sort_by(|&a, &b| {
        let (ea, eb) = (&entries[a], &entries[b]);
        let ord = match key {
            SortKey::Status => {
                apply_direction(ea.status.cmp(&eb.status), descending)
            }
            SortKey::Name => {
                apply_direction(name_cmp(&ea.name, &eb.name), descending)
            }
            SortKey::SizeLeft => option_cmp(
                side_size(ea, true),
                side_size(eb, true),
                descending,
            ),
            SortKey::SizeRight => option_cmp(
                side_size(ea, false),
                side_size(eb, false),
                descending,
            ),
            SortKey::DateLeft => option_cmp(
                side_modified(ea, true),
                side_modified(eb, true),
                descending,
            ),
            SortKey::DateRight => option_cmp(
                side_modified(ea, false),
                side_modified(eb, false),
                descending,
            ),
        };
        ord.then_with(|| name_cmp(&ea.name, &eb.name))
    });
    order
}

/// Reverse `ord` when sorting descending.
fn apply_direction(ord: Ordering, descending: bool) -> Ordering {
    if descending { ord.reverse() } else { ord }
}

/// Case-insensitive name ordering.
fn name_cmp(a: &str, b: &str) -> Ordering {
    a.to_lowercase().cmp(&b.to_lowercase())
}

/// Compare optional side values; a missing side always sorts last.
fn option_cmp<T: Ord>(
    a: Option<T>,
    b: Option<T>,
    descending: bool,
) -> Ordering {
    match (a, b) {
        (Some(x), Some(y)) => apply_direction(x.cmp(&y), descending),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

fn side_size(entry: &DirEntry, left: bool) -> Option<u64> {
    let side = if left { &entry.left } else { &entry.right };
    side.as_ref().map(|s| s.size)
}

fn side_modified(entry: &DirEntry, left: bool) -> Option<&str> {
    let side = if left { &entry.left } else { &entry.right };
    side.as_ref().map(|s| s.modified.as_str())
}

/// Absolute endpoints for an entry's two sides.
///
/// An entry's side `path` is relative to the comparison root, so it is
/// joined onto the root to obtain the absolute endpoint; a side missing
/// the entry falls back to the entry name.
pub(crate) fn entry_endpoints(
    entry: &DirEntry,
    left_root: &Endpoint,
    right_root: &Endpoint,
) -> (Endpoint, Endpoint) {
    let left = left_root.join(
        entry
            .left
            .as_ref()
            .map_or(entry.name.as_str(), |l| l.path.as_str()),
    );
    let right = right_root.join(
        entry
            .right
            .as_ref()
            .map_or(entry.name.as_str(), |r| r.path.as_str()),
    );
    (left, right)
}

/// One entered directory of the paired navigation, with the per-side
/// relative path segments.
///
/// A side's `path` is relative to the current comparison root and the two
/// sides may address the same entry through different paths, so each step
/// records both segments. The label is the entry name shown in the
/// breadcrumb.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NavStep {
    /// Entry name, used as the breadcrumb label.
    pub label: SharedString,
    /// Left-side relative path segment.
    pub left: String,
    /// Right-side relative path segment.
    pub right: String,
}

/// The compared endpoints at a given navigation depth.
///
/// Pure helper over the session roots and the entered steps, so endpoint
/// recomputation is testable without triggering a comparison load.
fn endpoints_at_depth(
    root_left: &Endpoint,
    root_right: &Endpoint,
    steps: &[NavStep],
) -> (Endpoint, Endpoint) {
    let mut left = root_left.clone();
    let mut right = root_right.clone();
    for step in steps {
        left = left.join(&step.left);
        right = right.join(&step.right);
    }
    (left, right)
}

/// Build the navigation step recorded for entering `entry`.
fn nav_step_for(entry: &DirEntry) -> NavStep {
    NavStep {
        label: entry.name.as_str().into(),
        left: entry
            .left
            .as_ref()
            .map_or(entry.name.as_str(), |l| l.path.as_str())
            .to_string(),
        right: entry
            .right
            .as_ref()
            .map_or(entry.name.as_str(), |r| r.path.as_str())
            .to_string(),
    }
}

/// A restorable navigation position in the session's back/forward
/// history: the compared base-folder pair and the directories entered
/// below it.
#[derive(Clone, Debug)]
struct HistoryEntry {
    root_left: Endpoint,
    root_right: Endpoint,
    nav_steps: Vec<NavStep>,
}

/// Which side(s) a "set as base folder" command re-roots.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BaseSide {
    /// Re-root the left side only.
    Left,
    /// Re-root the right side only.
    Right,
    /// Re-root both sides at the selected entry.
    Both,
}

/// A transfer the UI can run on the selected entry, with the labels and
/// the confirmation requirement derived from the entry's shape.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TransferPlan {
    /// The transfer item to execute.
    pub(crate) item: TransferItem,
    /// Whether the operation overwrites or destroys existing content and
    /// therefore needs an explicit confirmation before running.
    pub(crate) confirm: bool,
    /// Label shown while the operation runs.
    pub(crate) busy: &'static str,
    /// Label prefix shown when the operation completes.
    pub(crate) done: &'static str,
}

/// Build the transfer plan for `action` on `entry`, if the entry's shape
/// allows it.
///
/// The action names the destination side: `CopyLeft`/`MoveLeft` take the
/// right side as source, `CopyRight`/`MoveRight` the left side, and the
/// deletes remove the entry from the named side. An action is offered
/// whenever its source (or delete target) exists; it requires
/// confirmation whenever it overwrites an existing target or deletes
/// content.
pub(crate) fn transfer_plan_for(
    entry: &DirEntry,
    action: TransferAction,
) -> Option<TransferPlan> {
    if entry.name_conflict {
        return None;
    }
    let (source, overwrite, busy, done) = match action {
        TransferAction::CopyLeft => (
            entry.right.as_ref()?,
            entry.left.is_some(),
            "Copying to left…",
            "Copied to the left",
        ),
        TransferAction::CopyRight => (
            entry.left.as_ref()?,
            entry.right.is_some(),
            "Copying to right…",
            "Copied to the right",
        ),
        TransferAction::MoveLeft => (
            entry.right.as_ref()?,
            entry.left.is_some(),
            "Moving to left…",
            "Moved to the left",
        ),
        TransferAction::MoveRight => (
            entry.left.as_ref()?,
            entry.right.is_some(),
            "Moving to right…",
            "Moved to the right",
        ),
        TransferAction::DeleteLeft => (
            entry.left.as_ref()?,
            true,
            "Deleting from left…",
            "Deleted from the left",
        ),
        TransferAction::DeleteRight => (
            entry.right.as_ref()?,
            true,
            "Deleting from right…",
            "Deleted from the right",
        ),
        // 3-way merge actions belong to the merge view.
        TransferAction::CopyCenter => return None,
    };
    // Identical entries need no reconciliation; copying them would only
    // rewrite equal content.
    if matches!(
        entry.status,
        DirEntryStatus::Same | DirEntryStatus::SameBinary
    ) {
        return None;
    }
    Some(TransferPlan {
        item: TransferItem::new(
            action,
            entry.name.clone(),
            source.is_dir,
            source.path.clone(),
        ),
        confirm: overwrite,
        busy,
        done,
    })
}

// ---------------------------------------------------------------------------
// Application state
// ---------------------------------------------------------------------------

/// Application state that holds the comparison results.
pub struct AppState {
    /// Focus handle for keyboard navigation.
    focus_handle: FocusHandle,
    /// Window title.
    title: SharedString,
    /// Left-side endpoint (directory).
    left: Endpoint,
    /// Right-side endpoint (directory).
    right: Endpoint,
    /// Comparison result, or `None` if not yet loaded.
    comparison: Option<DirComparison>,
    /// Active column sort key.
    sort_key: SortKey,
    /// Whether the active sort is descending.
    sort_descending: bool,
    /// Entry indices in display order (positions into `comparison.entries`).
    ///
    /// All row access and selection goes through this map so sorting and
    /// filtering never reorder the underlying comparison data.
    display_order: Vec<usize>,
    /// Which entry categories the table displays.
    filters: DisplayFilters,
    /// Selected row index in display order.
    selected_index: usize,
    /// Whether a comparison or transfer is currently in progress.
    loading: bool,
    /// Label for the in-flight operation, shown while `loading`.
    busy_label: Option<SharedString>,
    /// Outcome of the last completed transfer, shown until the next
    /// navigation or operation.
    notice: Option<SharedString>,
    /// Error message, if any.
    error: Option<String>,
    /// Session root of the left side; the breadcrumb starts here.
    root_left: Endpoint,
    /// Session root of the right side.
    root_right: Endpoint,
    /// Directories entered since the session roots, oldest first.
    nav_steps: Vec<NavStep>,
    /// Per-session comparison settings.
    settings: SessionSettings,
    /// Current name-filter pattern shown in the toolbar's Filters combo.
    ///
    /// Kept apart from the settings history so an empty (cleared) filter
    /// does not pollute the history list.
    filter_current: Option<String>,
    /// Session type.
    session_type: SessionType,
    /// Monotonic counter that invalidates results of stopped or
    /// superseded comparison loads.
    load_revision: u64,
    /// Cancellation token of the in-flight comparison, if any.
    cancel_token: Option<CancellationToken>,
    /// Hash cache shared by the metadata scan, the background content
    /// pass, and later reloads of this session.
    content_cache: Arc<ContentCache>,
    /// Back/forward history of compared base-folder pairs, oldest first.
    history: Vec<HistoryEntry>,
    /// Position of the current state within `history`.
    history_pos: usize,
}

/// The history entry recorded for a freshly opened session root pair.
fn initial_history(left: &Endpoint, right: &Endpoint) -> Vec<HistoryEntry> {
    vec![HistoryEntry {
        root_left: left.clone(),
        root_right: right.clone(),
        nav_steps: Vec::new(),
    }]
}

// TODO: remove this
#[allow(dead_code)]
impl AppState {
    /// Create a new application state for the given endpoints.
    pub fn new(
        left: Endpoint,
        right: Endpoint,
        title: SharedString,
        cx: &mut Context<Self>,
    ) -> Self {
        let history = initial_history(&left, &right);
        Self {
            focus_handle: cx.focus_handle(),
            title,
            left: left.clone(),
            right: right.clone(),
            comparison: None,
            sort_key: SortKey::Name,
            sort_descending: false,
            display_order: Vec::new(),
            filters: DisplayFilters::default(),
            selected_index: 0,
            loading: false,
            busy_label: None,
            notice: None,
            error: None,
            root_left: left,
            root_right: right,
            nav_steps: Vec::new(),
            settings: SessionSettings::default(),
            filter_current: None,
            session_type: SessionType::DirCompare,
            load_revision: 0,
            cancel_token: None,
            content_cache: Arc::new(ContentCache::default_config()),
            history,
            history_pos: 0,
        }
    }

    /// Create a new application state from a session config whose
    /// endpoints were rebuilt from the config's provider refs.
    pub fn from_config(
        config: &SessionConfig,
        left: Endpoint,
        right: Endpoint,
        title: SharedString,
        cx: &mut Context<Self>,
    ) -> Self {
        let history = initial_history(&left, &right);
        Self {
            focus_handle: cx.focus_handle(),
            title,
            left: left.clone(),
            right: right.clone(),
            comparison: None,
            sort_key: SortKey::Name,
            sort_descending: false,
            display_order: Vec::new(),
            filters: DisplayFilters::default(),
            selected_index: 0,
            loading: false,
            busy_label: None,
            notice: None,
            error: None,
            root_left: left,
            root_right: right,
            nav_steps: Vec::new(),
            settings: config.settings.clone(),
            filter_current: config.settings.name_filter.last().cloned(),
            session_type: config.session_type,
            load_revision: 0,
            cancel_token: None,
            content_cache: Arc::new(ContentCache::default_config()),
            history,
            history_pos: 0,
        }
    }

    // -----------------------------------------------------------------------
    // Public accessors
    // -----------------------------------------------------------------------

    /// Return the focus handle for keyboard navigation.
    pub fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }

    /// Return the window title.
    pub fn title(&self) -> &SharedString {
        &self.title
    }

    /// Return the left-side endpoint.
    pub fn left(&self) -> &Endpoint {
        &self.left
    }

    /// Return the right-side endpoint.
    pub fn right(&self) -> &Endpoint {
        &self.right
    }

    /// Return the comparison result, if loaded.
    pub fn comparison(&self) -> Option<&DirComparison> {
        self.comparison.as_ref()
    }

    /// Return the selected row index in display order.
    pub fn selected_index(&self) -> usize {
        self.selected_index
    }

    /// Return the active column sort key.
    pub fn sort_key(&self) -> SortKey {
        self.sort_key
    }

    /// Whether the active sort is descending.
    pub fn is_sort_descending(&self) -> bool {
        self.sort_descending
    }

    /// Return the active display filters.
    pub fn filters(&self) -> DisplayFilters {
        self.filters
    }

    /// Return the directories entered since the session roots.
    pub fn nav_steps(&self) -> &[NavStep] {
        &self.nav_steps
    }

    /// Return the left session root (the breadcrumb's first crumb).
    pub fn root_left(&self) -> &Endpoint {
        &self.root_left
    }

    /// Return the right session root.
    pub fn root_right(&self) -> &Endpoint {
        &self.root_right
    }

    /// Return the label for the in-flight operation, if any.
    pub fn busy_label(&self) -> Option<&str> {
        self.busy_label.as_deref()
    }

    /// Return the outcome of the last completed transfer, if any.
    pub fn notice(&self) -> Option<&str> {
        self.notice.as_deref()
    }

    /// Whether a comparison is currently in progress.
    pub fn is_loading(&self) -> bool {
        self.loading
    }

    /// Return the error message, if any.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Return the entry at a display-order position, if any.
    pub fn entry_at(&self, display_index: usize) -> Option<&DirEntry> {
        let &entry_index = self.display_order.get(display_index)?;
        self.comparison.as_ref()?.entries.get(entry_index)
    }

    /// Return the currently selected entry, if any.
    pub fn selected_entry(&self) -> Option<&DirEntry> {
        self.entry_at(self.selected_index)
    }

    /// Display path of the selected entry's left side, for the clipboard.
    pub fn selected_left_path(&self) -> Option<String> {
        let entry = self.selected_entry()?;
        let (left, _) = entry_endpoints(entry, &self.left, &self.right);
        Some(left.display())
    }

    /// Display path of the selected entry's right side, for the clipboard.
    pub fn selected_right_path(&self) -> Option<String> {
        let entry = self.selected_entry()?;
        let (_, right) = entry_endpoints(entry, &self.left, &self.right);
        Some(right.display())
    }

    /// Return the number of entries in the comparison.
    pub fn entry_count(&self) -> usize {
        self.display_order.len()
    }

    /// Summary counts over the visible (filtered) entries, so the status
    /// bar describes what the table actually shows.
    pub fn visible_summary(&self) -> StatusSummary {
        let Some(comparison) = self.comparison.as_ref() else {
            return StatusSummary::default();
        };
        StatusSummary::from_entries(
            self.display_order.iter().map(|&i| &comparison.entries[i]),
        )
    }

    /// Return the session type.
    #[allow(dead_code)]
    pub fn session_type(&self) -> SessionType {
        self.session_type
    }

    /// Return the session settings.
    #[allow(dead_code)]
    pub fn settings(&self) -> &SessionSettings {
        &self.settings
    }

    /// Convert the current state to a serializable session config.
    pub fn to_config(&self, name: String) -> SessionConfig {
        SessionConfig {
            name,
            session_type: self.session_type,
            left: ProviderRef {
                provider: self.left.provider_ref(),
                path: self.left.path().to_path_buf(),
            },
            right: ProviderRef {
                provider: self.right.provider_ref(),
                path: self.right.path().to_path_buf(),
            },
            center: None,
            settings: self.settings.clone(),
        }
    }

    // -----------------------------------------------------------------------
    // Mutating operations (called via entity.update())
    // -----------------------------------------------------------------------

    /// Load the comparison asynchronously.
    ///
    /// The load runs in two passes when content comparison is enabled:
    /// a fast metadata scan whose result is displayed immediately, then
    /// a background content pass that refines equal-size file pairs by
    /// hashing. Results of a stopped or superseded load are dropped via
    /// the revision guard, so the previously displayed comparison stays
    /// visible until fresh results arrive.
    pub fn load_comparison(&mut self, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }

        self.loading = true;
        self.busy_label = Some("scanning…".into());
        self.error = None;
        self.load_revision += 1;
        let revision = self.load_revision;
        let token = CancellationToken::new();
        self.cancel_token = Some(token.clone());
        cx.notify();

        let left = self.left.clone();
        let right = self.right.clone();
        let left_display = left.display();
        let right_display = right.display();
        let compare_files = self.settings.compare_files;
        let cache = self.content_cache.clone();
        let weak = cx.entity().downgrade();

        log::push(
            LogLevel::Info,
            format!("Load comparison: {left_display} ↔ {right_display}"),
            cx,
        );

        // The metadata pass always runs metadata-only; content hashing
        // is the second pass below. Spawned on the tokio runtime because
        // the providers use tokio::fs internally and require a tokio
        // reactor. Both sides on the same provider instance compare
        // through the single-filesystem path; a mixed pair compares
        // across providers.
        let scan_token = token.clone();
        let join_handle = crate::runtime::spawn_on_tokio(async move {
            let config = CompareConfig::meta_data_only();
            let comparison = if left.same_provider(&right) {
                compare_directories_node(
                    left.fs(),
                    left.path(),
                    right.path(),
                    &config,
                    Some(&cache),
                    Some(&scan_token),
                )
                .await
            } else {
                compare_directories_pair_node(
                    left.fs(),
                    right.fs(),
                    left.path(),
                    right.path(),
                    &config,
                    Some(&cache),
                    Some(&scan_token),
                )
                .await
            };
            comparison.map_err(|e| anyhow::anyhow!("{e}"))
        });

        // Await the JoinHandle inside a gpui Task. Awaiting a JoinHandle
        // does not require a tokio reactor — it just polls the task running
        // on tokio's executor.
        cx.spawn(move |_, cx: &mut gpui::AsyncApp| {
            let async_app = cx.clone();
            async move {
                let result = match join_handle.await {
                    Ok(r) => r,
                    Err(e) => Err(anyhow::anyhow!("task panicked: {e}")),
                };
                async_app.update(|cx| {
                    let Some(state) = weak.upgrade() else { return };
                    state.update(cx, |state, cx| {
                        // Drop results of a load that was stopped or
                        // superseded; the stop path already logged.
                        if revision != state.load_revision {
                            return;
                        }
                        match result {
                            Ok(comparison) => {
                                let count = comparison.entries.len();
                                state.comparison = Some(comparison);
                                state.recompute_display_order();
                                state.selected_index = 0;
                                log::push(
                                    LogLevel::Info,
                                    format!(
                                        "Comparison completed: {count} \
                                         entries"
                                    ),
                                    cx,
                                );
                                if compare_files {
                                    // The content pass takes over the
                                    // cancellation token and the loading
                                    // flag.
                                    state.start_content_pass(revision, cx);
                                } else {
                                    state.cancel_token = None;
                                    state.loading = false;
                                    state.busy_label = None;
                                    cx.notify();
                                }
                            }
                            Err(e) => {
                                state.cancel_token = None;
                                state.loading = false;
                                state.busy_label = None;
                                state.error =
                                    Some(format!("Comparison failed: {e}"));
                                log::push(
                                    LogLevel::Error,
                                    format!("Comparison failed: {e}"),
                                    cx,
                                );
                                cx.notify();
                            }
                        }
                    });
                });
            }
        })
        .detach();
    }

    /// Refine the displayed comparison with content hashes in the
    /// background.
    ///
    /// Runs [`compare_content_pair_node`] on the tokio runtime over the
    /// currently displayed comparison, reporting `comparing content
    /// N/M` progress into [`Self::busy_label`] until the pass finishes.
    /// The refined comparison replaces the displayed one with the row
    /// selection following the same entry by name.
    fn start_content_pass(&mut self, revision: u64, cx: &mut Context<Self>) {
        let (Some(comparison), Some(token)) =
            (self.comparison.clone(), self.cancel_token.clone())
        else {
            self.cancel_token = None;
            self.loading = false;
            self.busy_label = None;
            cx.notify();
            return;
        };
        // Keep `loading` set: the status bar shows the progress label
        // and transfers wait for the pass to finish.
        self.busy_label = Some("comparing content…".into());
        cx.notify();

        log::push(LogLevel::Info, "Background content comparison started", cx);
        let started = Instant::now();

        let left = self.left.clone();
        let right = self.right.clone();
        let cache = self.content_cache.clone();
        let weak = cx.entity().downgrade();

        // Progress flows back on an unbounded channel; the sending side
        // throttles so a large tree does not flood the main thread.
        let (tx, mut rx) =
            tokio::sync::mpsc::unbounded_channel::<(usize, usize)>();
        let last_emit = Arc::new(std::sync::Mutex::new(Instant::now()));

        let content_token = token.clone();
        let join_handle = crate::runtime::spawn_on_tokio(async move {
            let on_progress = move |done: usize, total: usize| {
                let emit = done == total || {
                    let mut last = last_emit.lock().unwrap();
                    if last.elapsed().as_millis() >= 100 {
                        *last = Instant::now();
                        true
                    } else {
                        false
                    }
                };
                if emit {
                    let _ = tx.send((done, total));
                }
            };
            // The pair function handles a shared provider instance
            // exactly like the single-fs wrapper, so one call covers
            // both endpoint shapes.
            let result = compare_content_pair_node(
                left.fs(),
                right.fs(),
                left.path(),
                right.path(),
                comparison,
                Some(&cache),
                Some(&content_token),
                &on_progress,
            )
            .await;
            result.map_err(|e| anyhow::anyhow!("{e}"))
        });

        // Drain progress events until the task finishes and the channel
        // closes. Awaiting `recv` needs no tokio reactor; it is a plain
        // waker future.
        let progress_weak = weak.clone();
        cx.spawn(move |_, cx: &mut gpui::AsyncApp| {
            let async_app = cx.clone();
            async move {
                while let Some((done, total)) = rx.recv().await {
                    async_app.update(|cx| {
                        let Some(state) = progress_weak.upgrade() else {
                            return;
                        };
                        state.update(cx, |state, cx| {
                            // Ignore late events after stop or reload.
                            if revision != state.load_revision
                                || !state.loading
                            {
                                return;
                            }
                            state.busy_label = Some(
                                format!("comparing content {done}/{total}")
                                    .into(),
                            );
                            cx.notify();
                        });
                    });
                }
            }
        })
        .detach();

        let content_weak = weak.clone();
        cx.spawn(move |_, cx: &mut gpui::AsyncApp| {
            let async_app = cx.clone();
            async move {
                let result = match join_handle.await {
                    Ok(r) => r,
                    Err(e) => Err(anyhow::anyhow!("task panicked: {e}")),
                };
                async_app.update(|cx| {
                    let Some(state) = content_weak.upgrade() else {
                        return;
                    };
                    state.update(cx, |state, cx| {
                        // A stopped or superseded pass leaves the
                        // displayed comparison as it is.
                        if revision != state.load_revision {
                            return;
                        }
                        state.loading = false;
                        state.busy_label = None;
                        state.cancel_token = None;
                        match result {
                            Ok(comparison) => {
                                // Selection follows the same entry by
                                // name across the in-place refinement.
                                let selected_name = state
                                    .selected_entry()
                                    .map(|e| e.name.clone());
                                state.comparison = Some(comparison);
                                state.recompute_display_order();
                                state.selected_index = selected_name
                                    .as_deref()
                                    .and_then(|name| {
                                        state.display_index_of_name(name)
                                    })
                                    .unwrap_or(0);
                                log::push(
                                    LogLevel::Info,
                                    format!(
                                        "Background content comparison \
                                         completed in {:.1} s",
                                        started.elapsed().as_secs_f64()
                                    ),
                                    cx,
                                );
                            }
                            Err(e) => {
                                log::push(
                                    LogLevel::Error,
                                    format!("Content comparison failed: {e}"),
                                    cx,
                                );
                            }
                        }
                        cx.notify();
                    });
                });
            }
        })
        .detach();
    }

    /// Abort the in-flight comparison, keeping the currently displayed
    /// results visible.
    ///
    /// The cancellation token stops the scan or content pass on the
    /// tokio side; the revision guard drops whatever result the task
    /// still delivers.
    pub fn stop(&mut self, cx: &mut Context<Self>) {
        if !self.loading {
            return;
        }
        self.load_revision += 1;
        if let Some(token) = self.cancel_token.take() {
            token.cancel();
        }
        self.loading = false;
        self.busy_label = None;
        log::push(LogLevel::Info, "Comparison aborted", cx);
        cx.notify();
    }

    /// Select a row by display-order index.
    pub fn select_row(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.display_order.len() {
            self.selected_index = index;
            cx.notify();
        }
    }

    /// Navigate down by one row.
    pub fn select_next(&mut self, cx: &mut Context<Self>) {
        if self.selected_index + 1 < self.display_order.len() {
            self.selected_index += 1;
            cx.notify();
        }
    }

    /// Navigate up by one row.
    pub fn select_previous(&mut self, cx: &mut Context<Self>) {
        if self.selected_index > 0 {
            self.selected_index -= 1;
            cx.notify();
        }
    }

    /// Jump to the next entry that is not identical on both sides.
    ///
    /// Walks the visible display order (so filters apply) and wraps at
    /// the end. A no-op when the list holds no differences.
    pub fn select_next_difference(&mut self, cx: &mut Context<Self>) {
        self.select_difference(1, cx);
    }

    /// Jump to the previous entry that is not identical on both sides.
    pub fn select_previous_difference(&mut self, cx: &mut Context<Self>) {
        self.select_difference(-1, cx);
    }

    /// Move the selection `direction` rows to the next non-`Same` entry,
    /// wrapping around the visible list.
    fn select_difference(&mut self, direction: isize, cx: &mut Context<Self>) {
        let len = self.display_order.len() as isize;
        if len == 0 {
            return;
        }
        let Some(comparison) = self.comparison.as_ref() else {
            return;
        };
        let start = self.selected_index as isize;
        // A full turn covers every visible row, including wrapping back
        // to the start; landing on the current row is a deliberate no-op.
        for offset in 1..=len {
            let index = (start + direction * offset).rem_euclid(len) as usize;
            let status = comparison.entries[self.display_order[index]].status;
            if status_category(&status) != StatusCategory::Same {
                if index != self.selected_index {
                    self.selected_index = index;
                    cx.notify();
                }
                return;
            }
        }
    }

    /// Navigate into a subdirectory if the selected entry is a directory.
    pub fn navigate_into_subdir(&mut self, cx: &mut Context<Self>) {
        let entry = match self.selected_entry() {
            Some(e) => e,
            None => return,
        };

        // Only navigate into directories.
        if !(entry.left.as_ref().is_some_and(|l| l.is_dir)
            || entry.right.as_ref().is_some_and(|r| r.is_dir))
        {
            return;
        }

        // Determine sub-endpoints for both sides. An entry's `path` is
        // relative to the current comparison root, so it must be joined onto
        // the endpoint path rather than used as the endpoint path directly
        // (which would drop the root and resolve to a non-existent location).
        let step = nav_step_for(entry);
        let left_sub = self.left.join(&step.left);
        let right_sub = self.right.join(&step.right);

        self.nav_steps.push(step);
        self.left = left_sub;
        self.right = right_sub;
        self.notice = None;
        self.record_history();
        self.load_comparison(cx);
    }

    /// Navigate to parent directories.
    ///
    /// Inside the session this pops one breadcrumb step; at the session
    /// root it moves both sides one level up and re-roots the session,
    /// mirroring how a compare tool follows the user out of the pair.
    pub fn navigate_up(&mut self, cx: &mut Context<Self>) {
        if self.nav_steps.pop().is_some() {
            let (left, right) = endpoints_at_depth(
                &self.root_left,
                &self.root_right,
                &self.nav_steps,
            );
            self.left = left;
            self.right = right;
        } else if let (Some(lp), Some(rp)) =
            (self.left.parent(), self.right.parent())
        {
            self.left = lp;
            self.right = rp;
            self.root_left = self.left.clone();
            self.root_right = self.right.clone();
        } else {
            return;
        }
        self.notice = None;
        self.record_history();
        self.load_comparison(cx);
    }

    /// Navigate the pair to breadcrumb `depth` (0 is the session root).
    ///
    /// Clicks on the breadcrumb's current (last) crumb are ignored; the
    /// pair is already there.
    pub fn navigate_to_depth(&mut self, depth: usize, cx: &mut Context<Self>) {
        if depth >= self.nav_steps.len() {
            return;
        }
        self.nav_steps.truncate(depth);
        let (left, right) = endpoints_at_depth(
            &self.root_left,
            &self.root_right,
            &self.nav_steps,
        );
        self.left = left;
        self.right = right;
        self.notice = None;
        self.record_history();
        self.load_comparison(cx);
    }

    /// Swap the two sides of the comparison, keeping the navigation
    /// position: the entered steps stay, with their side segments
    /// exchanged.
    pub fn swap_sides(&mut self, cx: &mut Context<Self>) {
        self.swap_sides_in_place();
        self.notice = None;
        self.record_history();
        self.load_comparison(cx);
    }

    /// Exchange the sides' endpoints, roots, and step segments without
    /// triggering a reload.
    fn swap_sides_in_place(&mut self) {
        std::mem::swap(&mut self.left, &mut self.right);
        std::mem::swap(&mut self.root_left, &mut self.root_right);
        for step in &mut self.nav_steps {
            std::mem::swap(&mut step.left, &mut step.right);
        }
    }

    // -----------------------------------------------------------------------
    // Back/forward session history
    // -----------------------------------------------------------------------

    /// Record the current root pair and navigation steps as a new
    /// history entry, dropping any forward entries.
    fn record_history(&mut self) {
        self.history.truncate(self.history_pos + 1);
        self.history.push(HistoryEntry {
            root_left: self.root_left.clone(),
            root_right: self.root_right.clone(),
            nav_steps: self.nav_steps.clone(),
        });
        self.history_pos = self.history.len() - 1;
    }

    /// Whether the pair can step back to a previously compared state.
    pub fn can_go_back(&self) -> bool {
        self.history_pos > 0
    }

    /// Whether the pair can step forward after going back.
    pub fn can_go_forward(&self) -> bool {
        self.history_pos + 1 < self.history.len()
    }

    /// Step back to the previously compared base-folder pair.
    pub fn go_back(&mut self, cx: &mut Context<Self>) {
        if !self.can_go_back() {
            return;
        }
        self.history_pos -= 1;
        self.restore_history(cx);
    }

    /// Step forward to the next compared base-folder pair.
    pub fn go_forward(&mut self, cx: &mut Context<Self>) {
        if !self.can_go_forward() {
            return;
        }
        self.history_pos += 1;
        self.restore_history(cx);
    }

    /// Restore the roots and steps at `history_pos` and re-compare.
    fn restore_history(&mut self, cx: &mut Context<Self>) {
        let entry = self.history[self.history_pos].clone();
        self.root_left = entry.root_left;
        self.root_right = entry.root_right;
        self.nav_steps = entry.nav_steps;
        let (left, right) = endpoints_at_depth(
            &self.root_left,
            &self.root_right,
            &self.nav_steps,
        );
        self.left = left;
        self.right = right;
        self.notice = None;
        self.load_comparison(cx);
    }

    // -----------------------------------------------------------------------
    // Base folder re-rooting
    // -----------------------------------------------------------------------

    /// Whether the selected entry can serve as the base folder for
    /// `side`: the side must have the entry and it must be a directory.
    pub fn can_set_base_folder(&self, side: BaseSide) -> bool {
        let Some(entry) = self.selected_entry() else {
            return false;
        };
        let side_info = match side {
            BaseSide::Left => entry.left.as_ref(),
            BaseSide::Right => entry.right.as_ref(),
            BaseSide::Both => {
                return entry.left.as_ref().is_some_and(|l| l.is_dir)
                    && entry.right.as_ref().is_some_and(|r| r.is_dir);
            }
        };
        side_info.is_some_and(|i| i.is_dir)
    }

    /// Re-root the session at the selected entry for `side`, so the
    /// entry becomes the new base folder of the comparison. Navigation
    /// steps below the old roots are dropped; the re-rooted pair is
    /// recorded in the history so back/forward can undo it.
    pub fn set_base_folder(&mut self, side: BaseSide, cx: &mut Context<Self>) {
        if !self.can_set_base_folder(side) {
            return;
        }
        let Some(entry) = self.selected_entry() else {
            return;
        };
        let (left, right) = entry_endpoints(entry, &self.left, &self.right);
        match side {
            BaseSide::Left => self.root_left = left,
            BaseSide::Right => self.root_right = right,
            BaseSide::Both => {
                self.root_left = left;
                self.root_right = right;
            }
        }
        self.nav_steps.clear();
        self.left = self.root_left.clone();
        self.right = self.root_right.clone();
        self.notice = None;
        log::push(
            LogLevel::Info,
            format!(
                "Base folder set: {}",
                match side {
                    BaseSide::Left => self.root_left.display(),
                    BaseSide::Right => self.root_right.display(),
                    BaseSide::Both => {
                        format!(
                            "{} ↔ {}",
                            self.root_left.display(),
                            self.root_right.display()
                        )
                    }
                }
            ),
            cx,
        );
        self.record_history();
        self.load_comparison(cx);
    }

    /// Build the transfer plan for `action` on the selected entry.
    ///
    /// `None` means the entry's shape does not allow the action, which
    /// drives the enabled state of the menu and context-menu commands.
    pub fn transfer_plan(
        &self,
        action: TransferAction,
    ) -> Option<TransferPlan> {
        let entry = self.selected_entry()?;
        transfer_plan_for(entry, action)
    }

    /// Whether the selected entry can be reconciled with `action`, for
    /// the enabled state of the Actions menu commands.
    pub fn can_transfer(&self, action: TransferAction) -> bool {
        self.transfer_plan(action).is_some()
    }

    /// Surface why a reconcile command was not possible for the selected
    /// entry, so a shortcut on a blocked entry is not a silent no-op.
    ///
    /// A file-vs-directory name clash is the one blocked shape worth
    /// explaining; other blocked shapes are simply not offered.
    pub fn explain_blocked_transfer(&mut self, cx: &mut Context<Self>) {
        let conflict = self
            .selected_entry()
            .filter(|e| e.name_conflict)
            .map(|e| e.name.clone());
        if let Some(name) = conflict {
            self.notice = Some(
                format!(
                    "Skipped \"{name}\": a file and a directory share this \
                     name"
                )
                .into(),
            );
            cx.notify();
        }
    }

    /// Execute a previously planned transfer on the selected entry, then
    /// re-compare so the table reflects the transfer.
    ///
    /// Runs on the tokio runtime through the node-based provider API,
    /// which needs the tokio reactor, like the comparison itself.
    pub fn run_transfer(
        &mut self,
        plan: TransferPlan,
        cx: &mut Context<Self>,
    ) {
        if self.loading {
            return;
        }
        let TransferPlan {
            item, busy, done, ..
        } = plan;
        let name = item.name.clone();
        self.loading = true;
        self.busy_label = Some(busy.into());
        self.notice = None;
        cx.notify();

        // Both endpoints are moved into the task so the provider borrows
        // the `FsPair` addresses outlive the await.
        let left = self.left.clone();
        let right = self.right.clone();
        let join_handle = crate::runtime::spawn_on_tokio(async move {
            let pair = if left.same_provider(&right) {
                FsPair::Shared(left.fs())
            } else {
                FsPair::Separate(left.fs(), right.fs())
            };
            let result =
                execute_transfers(&[item], left.path(), right.path(), pair)
                    .await;
            if result.is_ok() {
                Ok(())
            } else {
                let details: Vec<String> =
                    result.errors.iter().map(|e| e.to_string()).collect();
                Err(anyhow::anyhow!(details.join("; ")))
            }
        });

        let weak = cx.entity().downgrade();
        cx.spawn(move |_, cx: &mut gpui::AsyncApp| {
            let async_app = cx.clone();
            async move {
                let result = match join_handle.await {
                    Ok(r) => r,
                    Err(e) => Err(anyhow::anyhow!("task panicked: {e}")),
                };
                async_app.update(|cx| {
                    if let Some(state) = weak.upgrade() {
                        state.update(cx, |state, cx| {
                            state.loading = false;
                            state.busy_label = None;
                            state.notice = Some(match result {
                                Ok(()) => {
                                    let message = format!("{done} \"{name}\"");
                                    log::push(
                                        LogLevel::Info,
                                        message.clone(),
                                        cx,
                                    );
                                    message.into()
                                }
                                Err(e) => {
                                    let message =
                                        format!("Transfer failed: {e}");
                                    log::push(
                                        LogLevel::Error,
                                        message.clone(),
                                        cx,
                                    );
                                    message.into()
                                }
                            });
                            // Re-compare so the reconciled entry shows as
                            // identical instead of stale.
                            state.load_comparison(cx);
                        });
                    }
                });
            }
        })
        .detach();
    }

    /// Reload the current comparison.
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        self.notice = None;
        self.load_comparison(cx);
    }

    /// Sort the entry table by `key`, toggling the direction when it is
    /// already the active key.
    ///
    /// Selection follows the same entry across the reorder (domain identity,
    /// not row index), matching the data-table guidance to keep selection
    /// stable across reordering.
    pub fn set_sort(&mut self, key: SortKey, cx: &mut Context<Self>) {
        if self.sort_key == key {
            self.sort_descending = !self.sort_descending;
        } else {
            self.sort_key = key;
            self.sort_descending = false;
        }
        let selected_name = self.selected_entry().map(|e| e.name.clone());
        self.recompute_display_order();
        self.selected_index = selected_name
            .as_deref()
            .and_then(|name| self.display_index_of_name(name))
            .unwrap_or(0);
        cx.notify();
    }

    /// Toggle the visibility of a status category.
    ///
    /// Selection follows the same entry across the change when it stays
    /// visible (domain identity, not row index), matching the guidance to
    /// preserve selection across filtering.
    pub fn toggle_filter(
        &mut self,
        category: StatusCategory,
        cx: &mut Context<Self>,
    ) {
        match category {
            StatusCategory::Same => self.filters.show_same ^= true,
            StatusCategory::Different => self.filters.show_different ^= true,
            StatusCategory::Orphan => self.filters.show_orphans ^= true,
        }
        let selected_name = self.selected_entry().map(|e| e.name.clone());
        self.recompute_display_order();
        self.selected_index = selected_name
            .as_deref()
            .and_then(|name| self.display_index_of_name(name))
            .unwrap_or(0);
        cx.notify();
    }

    /// The display mode the toolbar's exclusive toggle group shows.
    pub fn filter_mode(&self) -> FilterMode {
        let f = &self.filters;
        if f.all_visible() {
            FilterMode::All
        } else if !f.show_same && f.show_different && f.show_orphans {
            FilterMode::Diffs
        } else if f.show_same && !f.show_different && !f.show_orphans {
            FilterMode::Same
        } else {
            FilterMode::Custom
        }
    }

    /// Apply an exclusive display mode from the toolbar's toggle group.
    ///
    /// "Diffs" keeps both difference categories (different and orphan);
    /// a [`FilterMode::Custom`] combination is left untouched.
    pub fn set_filter_mode(
        &mut self,
        mode: FilterMode,
        cx: &mut Context<Self>,
    ) {
        let filters = match mode {
            FilterMode::All => DisplayFilters::default(),
            FilterMode::Diffs => DisplayFilters {
                show_same: false,
                show_different: true,
                show_orphans: true,
            },
            FilterMode::Same => DisplayFilters {
                show_same: true,
                show_different: false,
                show_orphans: false,
            },
            FilterMode::Custom => return,
        };
        if filters == self.filters {
            return;
        }
        self.filters = filters;
        let selected_name = self.selected_entry().map(|e| e.name.clone());
        self.recompute_display_order();
        self.selected_index = selected_name
            .as_deref()
            .and_then(|name| self.display_index_of_name(name))
            .unwrap_or(0);
        cx.notify();
    }

    /// The current name-filter pattern shown in the Filters combo.
    pub fn filter_pattern(&self) -> Option<&str> {
        self.filter_current.as_deref()
    }

    /// The entered filter patterns, oldest first (persisted in the
    /// session settings).
    pub fn filter_history(&self) -> &[String] {
        &self.settings.name_filter
    }

    /// Record `pattern` as the current filter and append it to the
    /// history stored in [`SessionSettings::name_filter`].
    ///
    /// An empty pattern clears the current filter but keeps the history,
    /// so the dropdown still offers the previous entries.
    pub fn push_filter_pattern(
        &mut self,
        pattern: &str,
        cx: &mut Context<Self>,
    ) {
        let pattern = pattern.trim();
        if self.filter_current.as_deref() == Some(pattern) {
            return;
        }
        self.filter_current = if pattern.is_empty() {
            None
        } else {
            Some(pattern.to_string())
        };
        if !pattern.is_empty()
            && self.settings.name_filter.last().map(String::as_str)
                != Some(pattern)
        {
            self.settings.name_filter.push(pattern.to_string());
        }
        if pattern.is_empty() {
            log::push(LogLevel::Info, "Filter cleared", cx);
        } else {
            log::push(LogLevel::Info, format!("Filter set: {pattern}"), cx);
        }
        cx.notify();
    }

    /// Display-order position of the entry with the given name.
    fn display_index_of_name(&self, name: &str) -> Option<usize> {
        let comparison = self.comparison.as_ref()?;
        self.display_order
            .iter()
            .position(|&i| comparison.entries[i].name == name)
    }

    /// Recompute the display order from the comparison, sort, and filters.
    fn recompute_display_order(&mut self) {
        self.display_order =
            self.comparison.as_ref().map_or_else(Vec::new, |c| {
                compute_display_order(
                    &c.entries,
                    self.sort_key,
                    self.sort_descending,
                    &self.filters,
                )
            });
    }

    /// Inject a comparison result without running the async load.
    #[cfg(test)]
    pub(crate) fn set_comparison_for_test(
        &mut self,
        comparison: DirComparison,
    ) {
        self.comparison = Some(comparison);
        self.recompute_display_order();
        self.selected_index = 0;
    }

    /// Whether the auto-load has been triggered.
    #[allow(dead_code)]
    pub fn auto_loaded(&self) -> bool {
        self.comparison.is_some() || self.loading || self.error.is_some()
    }

    /// Mark auto-load as triggered and start loading if not already done.
    pub fn trigger_auto_load(&mut self, cx: &mut Context<Self>) {
        if !self.loading && self.comparison.is_none() && self.error.is_none() {
            self.load_comparison(cx);
        }
    }
}

impl gpui::Focusable for AppState {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

// ---------------------------------------------------------------------------
// Status summary helpers
// ---------------------------------------------------------------------------

/// Summary counts for the comparison results.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StatusSummary {
    /// Total number of entries.
    pub total: usize,
    /// Number of identical entries.
    pub same: usize,
    /// Number of different entries.
    pub different: usize,
    /// Number of orphan entries (only on one side).
    pub orphans: usize,
}

impl StatusSummary {
    /// Compute status summary over an arbitrary set of entries, e.g. the
    /// visible rows of a filtered table.
    pub fn from_entries<'a>(
        entries: impl Iterator<Item = &'a DirEntry>,
    ) -> Self {
        let mut summary = Self::default();
        for entry in entries {
            summary.total += 1;
            match status_category(&entry.status) {
                StatusCategory::Same => summary.same += 1,
                StatusCategory::Different => summary.different += 1,
                StatusCategory::Orphan => summary.orphans += 1,
            }
        }
        summary
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use cocomo_lib::{DirComparison, EntryInfo};
    use gpui::{AppContext as _, TestAppContext};

    use super::*;

    fn info(path: &str, size: u64, modified: &str) -> EntryInfo {
        EntryInfo {
            name: path.rsplit('/').next().unwrap_or(path).to_string(),
            path: path.to_string(),
            size,
            modified: modified.to_string(),
            is_dir: false,
            hash: None,
        }
    }

    fn dir_info(path: &str) -> EntryInfo {
        EntryInfo {
            is_dir: true,
            ..info(path, 0, "")
        }
    }

    fn entry(
        name: &str,
        status: DirEntryStatus,
        left: Option<EntryInfo>,
        right: Option<EntryInfo>,
    ) -> DirEntry {
        DirEntry {
            name: name.to_string(),
            status,
            left,
            right,
            center: None,
            sub_entries: None,
            name_conflict: false,
        }
    }

    fn comparison(entries: Vec<DirEntry>) -> DirComparison {
        DirComparison {
            entries,
            counts: HashMap::new(),
            errors: Vec::new(),
        }
    }

    /// Install a tokio runtime handle so code paths that spawn through
    /// the runtime bridge work in tests. The runtime must stay alive for
    /// the test's duration, so the caller keeps the returned value.
    fn init_tokio_handle() -> tokio::runtime::Runtime {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        let _guard = rt.enter();
        crate::runtime::set_handle();
        rt
    }

    /// Register the log globals that `log::push` requires.
    fn init_log_globals(cx: &mut TestAppContext) {
        cx.update(|cx| {
            cx.set_global(crate::log::LogBuffer::default());
        });
    }

    fn sample_entries() -> Vec<DirEntry> {
        vec![
            entry(
                "delta.txt",
                DirEntryStatus::Different,
                Some(info("delta.txt", 300, "2026-01-03T10:00:00+00:00")),
                Some(info("delta.txt", 400, "2026-01-04T10:00:00+00:00")),
            ),
            entry(
                "Alpha.txt",
                DirEntryStatus::Same,
                Some(info("Alpha.txt", 100, "2026-01-01T10:00:00+00:00")),
                Some(info("Alpha.txt", 100, "2026-01-01T10:00:00+00:00")),
            ),
            entry(
                "charlie.txt",
                DirEntryStatus::LeftOnly,
                Some(info("charlie.txt", 200, "2026-01-02T10:00:00+00:00")),
                None,
            ),
            entry(
                "bravo.txt",
                DirEntryStatus::RightOnly,
                None,
                Some(info("bravo.txt", 500, "2026-01-05T10:00:00+00:00")),
            ),
        ]
    }

    fn names_in_order<'a>(
        entries: &'a [DirEntry],
        order: &[usize],
    ) -> Vec<&'a str> {
        order.iter().map(|&i| entries[i].name.as_str()).collect()
    }

    #[test]
    fn name_sort_is_case_insensitive_in_both_directions() {
        let entries = sample_entries();
        let filters = DisplayFilters::default();
        let asc =
            compute_display_order(&entries, SortKey::Name, false, &filters);
        assert_eq!(
            names_in_order(&entries, &asc),
            ["Alpha.txt", "bravo.txt", "charlie.txt", "delta.txt"]
        );
        let desc =
            compute_display_order(&entries, SortKey::Name, true, &filters);
        assert_eq!(
            names_in_order(&entries, &desc),
            ["delta.txt", "charlie.txt", "bravo.txt", "Alpha.txt"]
        );
    }

    #[test]
    fn status_sort_groups_by_status() {
        let entries = sample_entries();
        let asc = compute_display_order(
            &entries,
            SortKey::Status,
            false,
            &DisplayFilters::default(),
        );
        // DirEntryStatus order: Same < Different < LeftOnly < RightOnly.
        assert_eq!(
            names_in_order(&entries, &asc),
            ["Alpha.txt", "delta.txt", "charlie.txt", "bravo.txt"]
        );
    }

    #[test]
    fn size_sort_keeps_missing_sides_last_in_both_directions() {
        let entries = sample_entries();
        let filters = DisplayFilters::default();
        let asc = compute_display_order(
            &entries,
            SortKey::SizeLeft,
            false,
            &filters,
        );
        // Left sizes: 100, 200, 300; bravo.txt has no left side → last.
        assert_eq!(
            names_in_order(&entries, &asc),
            ["Alpha.txt", "charlie.txt", "delta.txt", "bravo.txt"]
        );
        let desc =
            compute_display_order(&entries, SortKey::SizeLeft, true, &filters);
        assert_eq!(
            names_in_order(&entries, &desc),
            ["delta.txt", "charlie.txt", "Alpha.txt", "bravo.txt"]
        );
    }

    #[test]
    fn date_sort_orders_by_right_side_modified() {
        let entries = sample_entries();
        let asc = compute_display_order(
            &entries,
            SortKey::DateRight,
            false,
            &DisplayFilters::default(),
        );
        // Right dates: Alpha 01-01, delta 01-04, bravo 01-05; charlie has
        // no right side → last.
        assert_eq!(
            names_in_order(&entries, &asc),
            ["Alpha.txt", "delta.txt", "bravo.txt", "charlie.txt"]
        );
    }

    #[test]
    fn filters_exclude_hidden_categories_from_the_display_order() {
        let entries = sample_entries();
        let filters = DisplayFilters {
            show_same: false,
            show_different: true,
            show_orphans: true,
        };
        let order =
            compute_display_order(&entries, SortKey::Name, false, &filters);
        assert_eq!(
            names_in_order(&entries, &order),
            ["bravo.txt", "charlie.txt", "delta.txt"]
        );
    }

    #[gpui::test]
    fn set_sort_toggles_direction_on_the_active_key(cx: &mut TestAppContext) {
        let state = cx.new(|cx| {
            let mut state = AppState::new(
                Endpoint::local("/left"),
                Endpoint::local("/right"),
                "test".into(),
                cx,
            );
            state.set_comparison_for_test(comparison(sample_entries()));
            state
        });

        state.update(cx, |state, cx| {
            // Default sort is ascending by name.
            assert_eq!(state.sort_key(), SortKey::Name);
            assert!(!state.is_sort_descending());

            state.set_sort(SortKey::Name, cx);
            assert_eq!(state.sort_key(), SortKey::Name);
            assert!(state.is_sort_descending());

            // Switching keys resets to ascending.
            state.set_sort(SortKey::Status, cx);
            assert_eq!(state.sort_key(), SortKey::Status);
            assert!(!state.is_sort_descending());
        });
    }

    #[gpui::test]
    fn set_sort_preserves_selection_by_entry_name(cx: &mut TestAppContext) {
        let state = cx.new(|cx| {
            let mut state = AppState::new(
                Endpoint::local("/left"),
                Endpoint::local("/right"),
                "test".into(),
                cx,
            );
            state.set_comparison_for_test(comparison(sample_entries()));
            state
        });

        state.update(cx, |state, cx| {
            // Name ascending: delta.txt is the last row.
            state.select_row(3, cx);
            assert_eq!(state.selected_entry().unwrap().name, "delta.txt");

            // Reversing the order moves delta.txt to the front; the
            // selection must follow the entry, not the row index.
            state.set_sort(SortKey::Name, cx);
            assert_eq!(state.selected_index(), 0);
            assert_eq!(state.selected_entry().unwrap().name, "delta.txt");
        });
    }

    #[gpui::test]
    fn entry_at_and_side_paths_map_through_the_display_order(
        cx: &mut TestAppContext,
    ) {
        let state = cx.new(|cx| {
            let mut state = AppState::new(
                Endpoint::local("/left"),
                Endpoint::local("/right"),
                "test".into(),
                cx,
            );
            state.set_comparison_for_test(comparison(sample_entries()));
            state
        });

        state.update(cx, |state, cx| {
            assert_eq!(state.entry_count(), 4);
            assert_eq!(state.entry_at(0).unwrap().name, "Alpha.txt");
            assert!(state.entry_at(4).is_none());

            state.select_row(3, cx);
            assert_eq!(
                state.selected_left_path().as_deref(),
                Some("/left/delta.txt")
            );
            assert_eq!(
                state.selected_right_path().as_deref(),
                Some("/right/delta.txt")
            );
        });
    }

    #[test]
    fn entry_endpoints_join_side_paths_onto_the_comparison_root() {
        let entry = entry(
            "sub",
            DirEntryStatus::Same,
            Some(info("sub", 1, "")),
            Some(info("other/sub", 1, "")),
        );
        let (left, right) = entry_endpoints(
            &entry,
            &Endpoint::local("/left"),
            &Endpoint::local("/right"),
        );
        assert_eq!(left.display(), "/left/sub");
        assert_eq!(right.display(), "/right/other/sub");
    }

    #[gpui::test]
    fn toggle_filter_hides_a_category_and_restores_selection(
        cx: &mut TestAppContext,
    ) {
        let state = cx.new(|cx| {
            let mut state = AppState::new(
                Endpoint::local("/left"),
                Endpoint::local("/right"),
                "test".into(),
                cx,
            );
            state.set_comparison_for_test(comparison(sample_entries()));
            state
        });

        state.update(cx, |state, cx| {
            // Name ascending: Alpha.txt (Same) is the first row.
            state.select_row(0, cx);
            state.toggle_filter(StatusCategory::Same, cx);

            assert_eq!(state.entry_count(), 3);
            assert!(!state.filters().all_visible());
            // The selected Same entry vanished; selection falls back to
            // the first visible row.
            assert_eq!(state.selected_index(), 0);
            assert_eq!(state.entry_at(0).unwrap().name, "bravo.txt");

            // A visible selection survives toggling another filter.
            state.select_row(2, cx);
            assert_eq!(state.selected_entry().unwrap().name, "delta.txt");
            state.toggle_filter(StatusCategory::Orphan, cx);
            assert_eq!(state.entry_count(), 1);
            assert_eq!(state.selected_entry().unwrap().name, "delta.txt");

            // Restoring both filters brings every row back.
            state.toggle_filter(StatusCategory::Orphan, cx);
            state.toggle_filter(StatusCategory::Same, cx);
            assert_eq!(state.entry_count(), 4);
            assert!(state.filters().all_visible());
        });
    }

    #[gpui::test]
    fn next_difference_walks_visible_differences_and_wraps(
        cx: &mut TestAppContext,
    ) {
        let state = cx.new(|cx| {
            let mut state = AppState::new(
                Endpoint::local("/left"),
                Endpoint::local("/right"),
                "test".into(),
                cx,
            );
            state.set_comparison_for_test(comparison(sample_entries()));
            state
        });

        state.update(cx, |state, cx| {
            // Name ascending: Alpha.txt (Same), bravo.txt (RightOnly),
            // charlie.txt (LeftOnly), delta.txt (Different).
            state.select_row(0, cx);

            state.select_next_difference(cx);
            assert_eq!(state.selected_index(), 1);
            state.select_next_difference(cx);
            assert_eq!(state.selected_index(), 2);
            state.select_next_difference(cx);
            assert_eq!(state.selected_index(), 3);
            // Wraps around to the first difference, skipping the leading
            // identical row.
            state.select_next_difference(cx);
            assert_eq!(state.selected_index(), 1);

            state.select_previous_difference(cx);
            assert_eq!(state.selected_index(), 3);
        });
    }

    #[gpui::test]
    fn difference_walking_respects_filters_and_noops_without_differences(
        cx: &mut TestAppContext,
    ) {
        // Only identical entries: walking must not move the selection.
        let state = cx.new(|cx| {
            let mut state = AppState::new(
                Endpoint::local("/left"),
                Endpoint::local("/right"),
                "test".into(),
                cx,
            );
            state.set_comparison_for_test(comparison(vec![entry(
                "a.txt",
                DirEntryStatus::Same,
                Some(info("a.txt", 1, "")),
                Some(info("a.txt", 1, "")),
            )]));
            state
        });
        state.update(cx, |state, cx| {
            state.select_next_difference(cx);
            assert_eq!(state.selected_index(), 0);
        });

        // With differences hidden, walking skips the hidden rows.
        let state = cx.new(|cx| {
            let mut state = AppState::new(
                Endpoint::local("/left"),
                Endpoint::local("/right"),
                "test".into(),
                cx,
            );
            state.set_comparison_for_test(comparison(sample_entries()));
            state
        });
        state.update(cx, |state, cx| {
            state.toggle_filter(StatusCategory::Orphan, cx);
            // Visible: Alpha.txt (Same), delta.txt (Different).
            state.select_row(0, cx);
            state.select_next_difference(cx);
            assert_eq!(state.selected_entry().unwrap().name, "delta.txt");
            // Wrapping skips the hidden orphans and the visible Same row.
            state.select_next_difference(cx);
            assert_eq!(state.selected_entry().unwrap().name, "delta.txt");
        });
    }

    #[gpui::test]
    fn swap_sides_exchanges_endpoints_roots_and_step_sides(
        cx: &mut TestAppContext,
    ) {
        let state = cx.new(|cx| {
            AppState::new(
                Endpoint::local("/left"),
                Endpoint::local("/right"),
                "test".into(),
                cx,
            )
        });

        state.update(cx, |state, _| {
            // Exercise the pure bookkeeping half of the swap; the reload
            // it triggers in the public method needs the tokio bridge and
            // is covered by the manual pass.
            state.nav_steps.push(NavStep {
                label: "sub".into(),
                left: "sub".into(),
                right: "other/sub".into(),
            });
            state.left = state.left.join("sub");
            state.right = state.right.join("other/sub");
            state.swap_sides_in_place();
            assert_eq!(state.left.display(), "/right/other/sub");
            assert_eq!(state.right.display(), "/left/sub");
            assert_eq!(state.root_left.display(), "/right");
            assert_eq!(state.root_right.display(), "/left");
            assert_eq!(state.nav_steps[0].left, "other/sub");
            assert_eq!(state.nav_steps[0].right, "sub");
        });
    }

    #[test]
    fn endpoints_at_depth_join_each_step_side() {
        let root_left = Endpoint::local("/left");
        let root_right = Endpoint::local("/right");
        let steps = vec![
            NavStep {
                label: "sub".into(),
                left: "sub".into(),
                right: "other/sub".into(),
            },
            NavStep {
                label: "inner".into(),
                left: "inner".into(),
                right: "inner".into(),
            },
        ];
        let (left, right) = endpoints_at_depth(&root_left, &root_right, &[]);
        assert_eq!(left.display(), "/left");
        assert_eq!(right.display(), "/right");
        let (left, right) =
            endpoints_at_depth(&root_left, &root_right, &steps);
        assert_eq!(left.display(), "/left/sub/inner");
        assert_eq!(right.display(), "/right/other/sub/inner");
        let (left, right) =
            endpoints_at_depth(&root_left, &root_right, &steps[..1]);
        assert_eq!(left.display(), "/left/sub");
        assert_eq!(right.display(), "/right/other/sub");
    }

    #[test]
    fn nav_step_records_both_side_paths() {
        let dir_entry = entry(
            "sub",
            DirEntryStatus::Same,
            Some(info("sub", 1, "")),
            Some(info("other/sub", 1, "")),
        );
        let step = nav_step_for(&dir_entry);
        assert_eq!(step.label, "sub");
        assert_eq!(step.left, "sub");
        assert_eq!(step.right, "other/sub");

        // A missing side falls back to the entry name.
        let orphan = entry(
            "only.txt",
            DirEntryStatus::LeftOnly,
            Some(info("only.txt", 1, "")),
            None,
        );
        let step = nav_step_for(&orphan);
        assert_eq!(step.right, "only.txt");
    }

    #[test]
    fn transfer_plan_maps_orphans_without_confirmation() {
        let right_only = entry(
            "new.txt",
            DirEntryStatus::RightOnly,
            None,
            Some(info("nested/new.txt", 7, "")),
        );
        let plan =
            transfer_plan_for(&right_only, TransferAction::CopyLeft).unwrap();
        assert_eq!(plan.item.action, TransferAction::CopyLeft);
        assert_eq!(plan.item.name, "new.txt");
        assert_eq!(plan.item.rel_path, "nested/new.txt");
        assert!(!plan.confirm);

        let left_only = entry(
            "old.txt",
            DirEntryStatus::LeftOnly,
            Some(info("old.txt", 3, "")),
            None,
        );
        let plan =
            transfer_plan_for(&left_only, TransferAction::CopyRight).unwrap();
        assert_eq!(plan.item.action, TransferAction::CopyRight);
        assert_eq!(plan.item.rel_path, "old.txt");
        assert!(!plan.confirm);

        // The reverse directions have no source on the missing side.
        assert!(
            transfer_plan_for(&right_only, TransferAction::CopyRight)
                .is_none()
        );
        assert!(
            transfer_plan_for(&left_only, TransferAction::CopyLeft).is_none()
        );
    }

    #[test]
    fn transfer_plan_offers_overwrite_copies_for_differing_entries() {
        let different = entry(
            "both.txt",
            DirEntryStatus::Different,
            Some(info("both.txt", 1, "")),
            Some(info("both.txt", 2, "")),
        );
        // Both copy directions exist and each overwrites the target, so
        // both require confirmation.
        let plan =
            transfer_plan_for(&different, TransferAction::CopyRight).unwrap();
        assert!(plan.confirm);
        assert_eq!(plan.item.rel_path, "both.txt");
        let plan =
            transfer_plan_for(&different, TransferAction::CopyLeft).unwrap();
        assert!(plan.confirm);

        // Moves and deletes are offered for differing entries and always
        // confirm.
        assert!(
            transfer_plan_for(&different, TransferAction::MoveRight)
                .unwrap()
                .confirm
        );
        assert!(
            transfer_plan_for(&different, TransferAction::DeleteLeft)
                .unwrap()
                .confirm
        );

        // Identical entries need no reconciliation at all.
        let same = entry(
            "same.txt",
            DirEntryStatus::Same,
            Some(info("same.txt", 1, "")),
            Some(info("same.txt", 1, "")),
        );
        assert!(transfer_plan_for(&same, TransferAction::CopyRight).is_none());
        assert!(
            transfer_plan_for(&same, TransferAction::DeleteLeft).is_none()
        );
    }

    #[test]
    fn transfer_plan_blocks_name_conflicts() {
        // Both halves of a file-vs-directory clash are blocked even though
        // their orphan statuses would map to a copy direction.
        let mut file_half = entry(
            "notes",
            DirEntryStatus::LeftOnly,
            Some(info("notes", 4, "")),
            None,
        );
        file_half.name_conflict = true;
        assert!(
            transfer_plan_for(&file_half, TransferAction::CopyRight).is_none()
        );

        let mut dir_half = entry(
            "notes",
            DirEntryStatus::RightOnly,
            None,
            Some(info("notes", 4096, "")),
        );
        dir_half.name_conflict = true;
        assert!(
            transfer_plan_for(&dir_half, TransferAction::CopyLeft).is_none()
        );
    }

    #[gpui::test]
    fn transfer_plan_of_a_mismatched_direction_is_none(
        cx: &mut TestAppContext,
    ) {
        let state = cx.new(|cx| {
            let mut state = AppState::new(
                Endpoint::local("/left"),
                Endpoint::local("/right"),
                "test".into(),
                cx,
            );
            state.set_comparison_for_test(comparison(sample_entries()));
            state
        });

        state.update(cx, |state, _cx| {
            // Name ascending: row 2 is charlie.txt (LeftOnly). Asking to
            // copy it to the left has no plan, so nothing can run in the
            // wrong direction.
            state.select_row(2, _cx);
            assert!(state.transfer_plan(TransferAction::CopyLeft).is_none());
            assert!(state.transfer_plan(TransferAction::CopyRight).is_some());
            assert!(!state.is_loading());
            assert!(state.busy_label().is_none());
            assert!(state.notice().is_none());
        });
    }

    #[gpui::test]
    fn stop_keeps_results_and_aborts_loading(cx: &mut TestAppContext) {
        init_log_globals(cx);
        let state = cx.new(|cx| {
            let mut state = AppState::new(
                Endpoint::local("/left"),
                Endpoint::local("/right"),
                "test".into(),
                cx,
            );
            state.set_comparison_for_test(comparison(sample_entries()));
            state
        });

        state.update(cx, |state, cx| {
            // Simulate an in-flight load without spawning the real task.
            state.loading = true;
            state.busy_label = Some("scanning…".into());
            state.cancel_token = Some(CancellationToken::new());
            let entries_before = state.entry_count();

            state.stop(cx);

            assert!(!state.is_loading());
            assert!(state.busy_label().is_none());
            // The previously displayed comparison stays visible.
            assert_eq!(state.entry_count(), entries_before);
        });
    }

    #[gpui::test]
    fn history_records_navigation_and_steps_back(cx: &mut TestAppContext) {
        // Navigation restores go through the tokio bridge, so the test
        // installs a runtime handle for the duration.
        let _rt = init_tokio_handle();
        init_log_globals(cx);
        let state = cx.new(|cx| {
            AppState::new(
                Endpoint::local("/left"),
                Endpoint::local("/right"),
                "test".into(),
                cx,
            )
        });

        state.update(cx, |state, _cx| {
            // A fresh session has nowhere to go back to.
            assert!(!state.can_go_back());
            assert!(!state.can_go_forward());

            // Recording a navigation step makes back available.
            state.nav_steps.push(NavStep {
                label: "sub".into(),
                left: "sub".into(),
                right: "sub".into(),
            });
            state.left = state.left.join("sub");
            state.right = state.right.join("sub");
            state.record_history();
            assert!(state.can_go_back());

            // Stepping back restores the session roots.
            state.history_pos -= 1;
            state.restore_history(_cx);
            assert_eq!(state.left().display(), "/left");
            assert_eq!(state.right().display(), "/right");
            assert!(state.nav_steps().is_empty());
            assert!(state.can_go_forward());

            // Stepping forward returns to the entered directory.
            state.history_pos += 1;
            state.restore_history(_cx);
            assert_eq!(state.left().display(), "/left/sub");
            assert_eq!(state.nav_steps().len(), 1);
        });
    }

    #[gpui::test]
    fn set_base_folder_reroots_the_selected_sides(cx: &mut TestAppContext) {
        let _rt = init_tokio_handle();
        init_log_globals(cx);
        let state = cx.new(|cx| {
            let mut state = AppState::new(
                Endpoint::local("/left"),
                Endpoint::local("/right"),
                "test".into(),
                cx,
            );
            state.set_comparison_for_test(comparison(vec![
                entry(
                    "both",
                    DirEntryStatus::Same,
                    Some(dir_info("both")),
                    Some(dir_info("other/both")),
                ),
                entry(
                    "lonely",
                    DirEntryStatus::LeftOnly,
                    Some(dir_info("lonely")),
                    None,
                ),
                entry(
                    "file.txt",
                    DirEntryStatus::Same,
                    Some(info("file.txt", 1, "")),
                    Some(info("file.txt", 1, "")),
                ),
            ]));
            state
        });

        state.update(cx, |state, _cx| {
            // Row 0 is the shared directory: all three re-roots apply.
            state.select_row(0, _cx);
            assert!(state.can_set_base_folder(BaseSide::Left));
            assert!(state.can_set_base_folder(BaseSide::Right));
            assert!(state.can_set_base_folder(BaseSide::Both));

            state.set_base_folder(BaseSide::Both, _cx);
            assert_eq!(state.root_left().display(), "/left/both");
            assert_eq!(state.root_right().display(), "/right/other/both");
            assert!(state.nav_steps().is_empty());
            // The re-root is recorded, so back can undo it.
            assert!(state.can_go_back());
        });

        state.update(cx, |state, _cx| {
            // Name ascending: row 2 is the left-only directory `lonely`,
            // so only the left re-root applies; row 1 is a file, where
            // none apply.
            state.go_back(_cx);
            state.select_row(2, _cx);
            assert!(state.can_set_base_folder(BaseSide::Left));
            assert!(!state.can_set_base_folder(BaseSide::Right));
            assert!(!state.can_set_base_folder(BaseSide::Both));
            state.select_row(1, _cx);
            assert!(!state.can_set_base_folder(BaseSide::Left));
        });
    }

    #[gpui::test]
    fn visible_summary_counts_only_visible_entries(cx: &mut TestAppContext) {
        let state = cx.new(|cx| {
            let mut state = AppState::new(
                Endpoint::local("/left"),
                Endpoint::local("/right"),
                "test".into(),
                cx,
            );
            state.set_comparison_for_test(comparison(sample_entries()));
            state
        });

        state.update(cx, |state, cx| {
            let summary = state.visible_summary();
            assert_eq!(
                (
                    summary.total,
                    summary.same,
                    summary.different,
                    summary.orphans
                ),
                (4, 1, 1, 2)
            );

            state.toggle_filter(StatusCategory::Same, cx);

            let summary = state.visible_summary();
            assert_eq!(
                (
                    summary.total,
                    summary.same,
                    summary.different,
                    summary.orphans
                ),
                (3, 0, 1, 2)
            );
        });
    }
}
