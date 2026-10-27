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

use std::cmp::Ordering;

use cocomo_lib::{
    CompareConfig, ContentCache, DirComparison, DirEntry, DirEntryStatus,
    Endpoint, ProviderRef, SessionConfig, SessionSettings, SessionType,
    compare_directories_node, compare_directories_pair_node,
};
use gpui::{App, Context, FocusHandle, SharedString};

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

/// Compute the display order (indices into `entries`) for a sort.
///
/// Missing side values always sort last regardless of direction, so an
/// orphan never jumps to the top of a size or date sort. Ties break on
/// name, which keeps the order stable and predictable across re-sorts.
fn compute_display_order(
    entries: &[DirEntry],
    key: SortKey,
    descending: bool,
) -> Vec<usize> {
    let mut order: Vec<usize> = (0..entries.len()).collect();
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
    /// All row access and selection goes through this map so sorting never
    /// reorders the underlying comparison data.
    display_order: Vec<usize>,
    /// Selected row index in display order.
    selected_index: usize,
    /// Whether a comparison is currently in progress.
    loading: bool,
    /// Error message, if any.
    error: Option<String>,
    /// Per-session comparison settings.
    settings: SessionSettings,
    /// Session type.
    session_type: SessionType,
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
        Self {
            focus_handle: cx.focus_handle(),
            title,
            left,
            right,
            comparison: None,
            sort_key: SortKey::Name,
            sort_descending: false,
            display_order: Vec::new(),
            selected_index: 0,
            loading: false,
            error: None,
            settings: SessionSettings::default(),
            session_type: SessionType::DirCompare,
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
        Self {
            focus_handle: cx.focus_handle(),
            title,
            left,
            right,
            comparison: None,
            sort_key: SortKey::Name,
            sort_descending: false,
            display_order: Vec::new(),
            selected_index: 0,
            loading: false,
            error: None,
            settings: config.settings.clone(),
            session_type: config.session_type,
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

    /// Update the endpoints from new values (e.g., after navigating).
    pub fn update_paths(&mut self, left: Endpoint, right: Endpoint) {
        self.left = left;
        self.right = right;
    }

    // -----------------------------------------------------------------------
    // Mutating operations (called via entity.update())
    // -----------------------------------------------------------------------

    /// Load the comparison asynchronously.
    pub fn load_comparison(&mut self, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }

        self.loading = true;
        self.error = None;
        self.comparison = None;
        self.display_order.clear();
        self.selected_index = 0;
        cx.notify();

        let left = self.left.clone();
        let right = self.right.clone();
        let title = self.title.clone();
        let left_display = left.display();
        let right_display = right.display();
        let compare_files = self.settings.compare_files;
        let weak = cx.entity().downgrade();

        // Spawn the comparison on the tokio runtime because the providers
        // use tokio::fs internally and require a tokio reactor. Both sides
        // on the same provider instance compare through the single-
        // filesystem path; a mixed pair compares across providers.
        let join_handle = crate::runtime::spawn_on_tokio(async move {
            let cache = ContentCache::default_config();
            let config = if compare_files {
                CompareConfig::full()
            } else {
                CompareConfig::meta_data_only()
            };

            if left.same_provider(&right) {
                compare_directories_node(
                    left.fs(),
                    left.path(),
                    right.path(),
                    &config,
                    Some(&cache),
                )
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))
            } else {
                compare_directories_pair_node(
                    left.fs(),
                    right.fs(),
                    left.path(),
                    right.path(),
                    &config,
                    Some(&cache),
                )
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))
            }
        });

        // Await the JoinHandle inside a gpui Task. Awaiting a JoinHandle
        // does not require a tokio reactor — it just polls the task running
        // on tokio's executor.
        cx.spawn(|_, cx: &mut gpui::AsyncApp| {
            let async_app = cx.clone();
            async move {
                let result = match join_handle.await {
                    Ok(r) => r,
                    Err(e) => Err(anyhow::anyhow!("task panicked: {e}")),
                };
                async_app.update(|cx| {
                    if let Some(state) = weak.upgrade() {
                        state.update(cx, |state, _| {
                            state.loading = false;
                            match result {
                                Ok(comparison) => {
                                    state.comparison = Some(comparison);
                                    state.recompute_display_order();
                                    state.selected_index = 0;
                                    state.title = SharedString::from(format!(
                                        "{title} — {left_display} vs \
                                         {right_display}"
                                    ));
                                }
                                Err(e) => {
                                    state.error = Some(format!(
                                        "Comparison failed: {e}"
                                    ));
                                }
                            }
                        });
                    }
                });
            }
        })
        .detach();
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
        let left_sub = match &entry.left {
            Some(left_info) => self.left.join(&left_info.path),
            None => self.left.join(&entry.name),
        };
        let right_sub = match &entry.right {
            Some(right_info) => self.right.join(&right_info.path),
            None => self.right.join(&entry.name),
        };

        self.left = left_sub;
        self.right = right_sub;
        self.load_comparison(cx);
    }

    /// Navigate to parent directories.
    pub fn navigate_up(&mut self, cx: &mut Context<Self>) {
        if let (Some(lp), Some(rp)) = (self.left.parent(), self.right.parent())
        {
            self.left = lp;
            self.right = rp;
            self.load_comparison(cx);
        }
    }

    /// Reload the current comparison.
    pub fn reload(&mut self, cx: &mut Context<Self>) {
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

    /// Display-order position of the entry with the given name.
    fn display_index_of_name(&self, name: &str) -> Option<usize> {
        let comparison = self.comparison.as_ref()?;
        self.display_order
            .iter()
            .position(|&i| comparison.entries[i].name == name)
    }

    /// Recompute the display order from the comparison and the active sort.
    fn recompute_display_order(&mut self) {
        self.display_order =
            self.comparison.as_ref().map_or_else(Vec::new, |c| {
                compute_display_order(
                    &c.entries,
                    self.sort_key,
                    self.sort_descending,
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
    /// Compute status summary from a comparison result.
    pub fn from_comparison(comparison: &DirComparison) -> Self {
        let total = comparison.entries.len();
        let same = comparison
            .entries
            .iter()
            .filter(|e| {
                matches!(
                    e.status,
                    DirEntryStatus::Same | DirEntryStatus::SameBinary
                )
            })
            .count();
        let different = comparison
            .entries
            .iter()
            .filter(|e| {
                matches!(
                    e.status,
                    DirEntryStatus::Different | DirEntryStatus::Similar
                )
            })
            .count();
        let orphans = comparison
            .entries
            .iter()
            .filter(|e| {
                matches!(
                    e.status,
                    DirEntryStatus::LeftOnly | DirEntryStatus::RightOnly
                )
            })
            .count();

        Self {
            total,
            same,
            different,
            orphans,
        }
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
        }
    }

    fn comparison(entries: Vec<DirEntry>) -> DirComparison {
        DirComparison {
            entries,
            counts: HashMap::new(),
            errors: Vec::new(),
        }
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
        let asc = compute_display_order(&entries, SortKey::Name, false);
        assert_eq!(
            names_in_order(&entries, &asc),
            ["Alpha.txt", "bravo.txt", "charlie.txt", "delta.txt"]
        );
        let desc = compute_display_order(&entries, SortKey::Name, true);
        assert_eq!(
            names_in_order(&entries, &desc),
            ["delta.txt", "charlie.txt", "bravo.txt", "Alpha.txt"]
        );
    }

    #[test]
    fn status_sort_groups_by_status() {
        let entries = sample_entries();
        let asc = compute_display_order(&entries, SortKey::Status, false);
        // DirEntryStatus order: Same < Different < LeftOnly < RightOnly.
        assert_eq!(
            names_in_order(&entries, &asc),
            ["Alpha.txt", "delta.txt", "charlie.txt", "bravo.txt"]
        );
    }

    #[test]
    fn size_sort_keeps_missing_sides_last_in_both_directions() {
        let entries = sample_entries();
        let asc = compute_display_order(&entries, SortKey::SizeLeft, false);
        // Left sizes: 100, 200, 300; bravo.txt has no left side → last.
        assert_eq!(
            names_in_order(&entries, &asc),
            ["Alpha.txt", "charlie.txt", "delta.txt", "bravo.txt"]
        );
        let desc = compute_display_order(&entries, SortKey::SizeLeft, true);
        assert_eq!(
            names_in_order(&entries, &desc),
            ["delta.txt", "charlie.txt", "Alpha.txt", "bravo.txt"]
        );
    }

    #[test]
    fn date_sort_orders_by_right_side_modified() {
        let entries = sample_entries();
        let asc = compute_display_order(&entries, SortKey::DateRight, false);
        // Right dates: Alpha 01-01, delta 01-04, bravo 01-05; charlie has
        // no right side → last.
        assert_eq!(
            names_in_order(&entries, &asc),
            ["Alpha.txt", "delta.txt", "bravo.txt", "charlie.txt"]
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
}
