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

use cocomo_lib::{
    CompareConfig, ContentCache, DirComparison, DirEntry, DirEntryStatus,
    Endpoint, ProviderRef, SessionConfig, SessionSettings, SessionType,
    compare_directories_node, compare_directories_pair_node,
};
use gpui::{App, Context, FocusHandle, SharedString};

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
        if let Some(comparison) = &self.comparison
            && index < comparison.entries.len()
        {
            self.selected_index = index;
            cx.notify();
        }
    }

    /// Navigate down by one row.
    pub fn select_next(&mut self, cx: &mut Context<Self>) {
        if let Some(comparison) = &self.comparison
            && self.selected_index + 1 < comparison.entries.len()
        {
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
