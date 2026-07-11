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
//! The [`AppState`] entity holds the left and right directory paths and the
//! results of the comparison. It provides methods to load and compare
//! directories asynchronously, and the UI observes this entity for updates.

use std::path::PathBuf;

use cocomo_lib::{
    CompareConfig, ContentCache, DirComparison, DirEntry, DirEntryStatus,
    LocalFs, ProviderRef, SessionConfig, SessionSettings, SessionType,
    compare_directories_node,
};
use gpui::{
    App, AppContext as _, Context, FocusHandle, SharedString, WeakEntity,
};

// ---------------------------------------------------------------------------
// Application state
// ---------------------------------------------------------------------------

/// Application state that holds the comparison results.
pub struct AppState {
    /// Focus handle for keyboard navigation.
    focus_handle: FocusHandle,
    /// Window title.
    title: SharedString,
    /// Left directory path.
    left_path: PathBuf,
    /// Right directory path.
    right_path: PathBuf,
    /// Comparison result, or `None` if not yet loaded.
    comparison: Option<DirComparison>,
    /// Selected row index.
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

impl AppState {
    /// Create a new application state with the given paths.
    pub fn new(
        left: PathBuf,
        right: PathBuf,
        title: SharedString,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            title,
            left_path: left,
            right_path: right,
            comparison: None,
            selected_index: 0,
            loading: false,
            error: None,
            settings: SessionSettings::default(),
            session_type: SessionType::DirCompare,
        }
    }

    /// Create a new application state from a session config.
    pub fn from_config(
        config: &SessionConfig,
        title: SharedString,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            title,
            left_path: config.left.path.clone(),
            right_path: config.right.path.clone(),
            comparison: None,
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

    /// Return the window title.
    #[allow(dead_code)]
    pub fn title(&self) -> &SharedString {
        &self.title
    }

    /// Return the left directory path.
    pub fn left_path(&self) -> &PathBuf {
        &self.left_path
    }

    /// Return the right directory path.
    pub fn right_path(&self) -> &PathBuf {
        &self.right_path
    }

    /// Return the comparison result, if loaded.
    pub fn comparison(&self) -> Option<&DirComparison> {
        self.comparison.as_ref()
    }

    /// Return the selected row index.
    pub fn selected_index(&self) -> usize {
        self.selected_index
    }

    /// Whether a comparison is currently in progress.
    pub fn is_loading(&self) -> bool {
        self.loading
    }

    /// Return the error message, if any.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Return the currently selected entry, if any.
    pub fn selected_entry(&self) -> Option<&DirEntry> {
        self.comparison
            .as_ref()
            .and_then(|c| c.entries.get(self.selected_index))
    }

    /// Return the number of entries in the comparison.
    pub fn entry_count(&self) -> usize {
        self.comparison.as_ref().map_or(0, |c| c.entries.len())
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
                provider: "local".to_string(),
                path: self.left_path.clone(),
            },
            right: ProviderRef {
                provider: "local".to_string(),
                path: self.right_path.clone(),
            },
            center: None,
            settings: self.settings.clone(),
        }
    }

    /// Update the paths from new values (e.g., after navigating).
    pub fn update_paths(&mut self, left: PathBuf, right: PathBuf) {
        self.left_path = left;
        self.right_path = right;
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
        self.selected_index = 0;
        cx.notify();

        let left = self.left_path.clone();
        let right = self.right_path.clone();
        let title = self.title.clone();
        let left_display = left.to_string_lossy().to_string();
        let right_display = right.to_string_lossy().to_string();
        let _entity = cx.entity().downgrade();

        // Spawn the comparison on a background thread.
        let task = cx.background_spawn(async move {
            let fs = LocalFs::new("local");
            let cache = ContentCache::default_config();
            let config = CompareConfig::full();

            compare_directories_node(&fs, &left, &right, &config, Some(&cache))
                .await
        });

        // Observe the task completion and update state when done.
        cx.spawn(|this: WeakEntity<AppState>, cx: &mut gpui::AsyncApp| {
            // Clone async_app OUTSIDE the async block so the async block
            // captures an owned AsyncApp, not a reference.
            let async_app = cx.clone();
            async move {
                let result = task.await;
                let _ = async_app.update(|cx| {
                    if let Some(state) = this.upgrade() {
                        let _ = state.update(cx, |state, _| {
                            state.loading = false;
                            match result {
                                Ok(comparison) => {
                                    state.comparison = Some(comparison);
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

    /// Select a row by index.
    #[allow(dead_code)]
    pub fn select_row(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(comparison) = &self.comparison {
            if index < comparison.entries.len() {
                self.selected_index = index;
                cx.notify();
            }
        }
    }

    /// Navigate down by one row.
    pub fn select_next(&mut self, cx: &mut Context<Self>) {
        if let Some(comparison) = &self.comparison {
            if self.selected_index + 1 < comparison.entries.len() {
                self.selected_index += 1;
                cx.notify();
            }
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
        let is_dir = entry.left.as_ref().map_or(false, |l| l.is_dir)
            || entry.right.as_ref().map_or(false, |r| r.is_dir);
        if !is_dir {
            return;
        }

        // Determine sub-paths for both sides.
        let left_sub = match &entry.left {
            Some(left_info) => PathBuf::from(&left_info.path),
            None => self.left_path.join(&entry.name),
        };
        let right_sub = match &entry.right {
            Some(right_info) => PathBuf::from(&right_info.path),
            None => self.right_path.join(&entry.name),
        };

        // Only navigate if at least one side is a directory.
        if !left_sub.is_dir() && !right_sub.is_dir() {
            return;
        }

        self.left_path = left_sub;
        self.right_path = right_sub;
        self.load_comparison(cx);
    }

    /// Navigate to parent directories.
    pub fn navigate_up(&mut self, cx: &mut Context<Self>) {
        let left_parent = self.left_path.parent().map(PathBuf::from);
        let right_parent = self.right_path.parent().map(PathBuf::from);

        match (left_parent, right_parent) {
            (Some(lp), Some(rp))
                if lp != self.left_path && rp != self.right_path =>
            {
                self.left_path = lp;
                self.right_path = rp;
                self.load_comparison(cx);
            }
            _ => {}
        }
    }

    /// Reload the current comparison.
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        self.load_comparison(cx);
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
