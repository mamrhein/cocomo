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
    Grammar, LineInfo, LocalFs, ProviderRef, SessionConfig, SessionSettings,
    SessionType as LibSessionType, SyncOperation, SyncResult, SyncRules,
    TextCompareSettings, TextDiff, compare_texts,
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
    /// The user is typing a session name to save.
    SaveSession,
    /// The user is browsing saved sessions to load one.
    LoadSession,
}

/// The type of content a session displays.
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
pub struct SessionView {
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
    /// Planned sync result for review. `Some` means sync preview is active.
    pub sync_planned: Option<SyncResult>,
    /// Sync operation currently configured.
    pub sync_operation: SyncOperation,
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
            text_diff: None,
            left_lines: Vec::new(),
            right_lines: Vec::new(),
            grammar: Grammar::plain_text(),
            table_state: TableState::default(),
            focus: Focus::default(),
            mode: AppMode::default(),
            filter_input: String::new(),
            active_filter: None,
            hide_same: false,
            compare_files,
            errors: Vec::new(),
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
            mode: AppMode::default(),
            filter_input: String::new(),
            active_filter: None,
            hide_same: false,
            compare_files: true,
            errors: Vec::new(),
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

/// Detect a suitable grammar for syntax classification based on file
/// extension.
pub fn detect_grammar(path: &Path) -> Grammar {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");

    match ext {
        "rs" => Grammar::rust(),
        "py" => Grammar::python(),
        "c" | "h" | "cpp" | "hpp" => Grammar::c(),
        _ => Grammar::plain_text(),
    }
}

/// Build the text compare settings using a detected grammar.
fn build_text_settings(grammar: &Grammar) -> TextCompareSettings {
    TextCompareSettings {
        grammar: Some(grammar.clone()),
        ..TextCompareSettings::default()
    }
}

/// Read a file and split it into line info entries with grammar
/// classification.
async fn read_file_lines(
    path: &Path,
    grammar: &Grammar,
) -> anyhow::Result<Vec<LineInfo>> {
    let content = tokio::fs::read_to_string(path).await?;
    let mut lines = Vec::new();
    for (i, line) in content.lines().enumerate() {
        lines.push(
            LineInfo::new(i + 1, line.to_string()).with_grammar(grammar),
        );
    }
    Ok(lines)
}

/// Run a text comparison and create a new [`SessionView`].
pub async fn run_text_comparison(
    left: PathBuf,
    right: PathBuf,
) -> anyhow::Result<SessionView> {
    // Detect grammar from the left file (fall back to right if left is
    // missing).
    let grammar = if left.exists() {
        detect_grammar(&left)
    } else {
        detect_grammar(&right)
    };

    // Read file contents.
    let left_content = if left.exists() {
        tokio::fs::read_to_string(&left).await.unwrap_or_default()
    } else {
        String::new()
    };
    let right_content = if right.exists() {
        tokio::fs::read_to_string(&right).await.unwrap_or_default()
    } else {
        String::new()
    };

    // Build line info for both sides.
    let left_lines = read_file_lines(&left, &grammar).await?;
    let right_lines = read_file_lines(&right, &grammar).await?;

    // Run the text comparison.
    let settings = build_text_settings(&grammar);
    let text_diff = compare_texts(&left_content, &right_content, &settings);

    Ok(SessionView::new_text_compare(
        left,
        right,
        left_lines,
        right_lines,
        text_diff,
        grammar,
    ))
}

/// Plan a sync operation (dry-run) and populate the session's sync state.
pub async fn plan_sync_session(
    session: &mut SessionView,
) -> anyhow::Result<()> {
    let fs = LocalFs::new("local");
    let rules = SyncRules {
        operation: session.sync_operation,
        dry_run: true,
        max_depth: None,
        compare_files: session.compare_files,
    };

    let result = cocomo_lib::plan_sync(
        &fs,
        &session.left_path,
        &session.right_path,
        &rules,
    )
    .await?;

    session.sync_planned = Some(result);
    session.table_state.select(Some(0));

    Ok(())
}

/// Execute the planned sync operation.
pub async fn execute_sync_session(
    session: &mut SessionView,
) -> anyhow::Result<()> {
    let fs = LocalFs::new("local");
    let rules = SyncRules {
        operation: session.sync_operation,
        dry_run: false,
        max_depth: None,
        compare_files: session.compare_files,
    };

    let result = cocomo_lib::sync_directories(
        &fs,
        &session.left_path,
        &session.right_path,
        &rules,
    )
    .await?;

    session.sync_planned = Some(result);
    session.table_state.select(Some(0));

    Ok(())
}

/// Convert a directory comparison session to a serializable config.
pub fn session_to_config(session: &SessionView) -> SessionConfig {
    SessionConfig {
        name: session.name.clone(),
        session_type: LibSessionType::DirCompare,
        left: ProviderRef {
            provider: "local".to_string(),
            path: session.left_path.clone(),
        },
        right: ProviderRef {
            provider: "local".to_string(),
            path: session.right_path.clone(),
        },
        center: None,
        settings: SessionSettings {
            compare_files: session.compare_files,
            compare_structure: true,
            name_filter: session
                .active_filter
                .as_ref()
                .map(|f| vec![f.clone()])
                .unwrap_or_default(),
            ignore_whitespace: false,
            skip_blank_lines: false,
            skip_comments: false,
            show_same: !session.hide_same,
            show_different: true,
            show_orphans: true,
        },
    }
}

/// List all saved session files in the default session directory.
pub async fn list_saved_sessions() -> anyhow::Result<Vec<PathBuf>> {
    let session_dir = default_session_dir();
    if !session_dir.exists() {
        return Ok(Vec::new());
    }

    let mut sessions = Vec::new();
    let mut entries = tokio::fs::read_dir(&session_dir).await?;
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "toml") {
            sessions.push(path);
        }
    }
    sessions.sort();
    Ok(sessions)
}

/// Return the default directory for saved session files.
pub fn default_session_dir() -> PathBuf {
    let config_dir = dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("~/.config"))
        .join("cocomo");
    config_dir.join("sessions")
}

/// Create a directory comparison session from a saved config.
pub async fn create_session_from_config(
    config: &SessionConfig,
) -> anyhow::Result<SessionView> {
    // Resolve the paths from the provider ref.
    let left_path = &config.left.path;
    let right_path = &config.right.path;

    let compare_files = config.settings.compare_files;
    let mut session = SessionView::new_dir_compare(
        left_path.clone(),
        right_path.clone(),
        compare_files,
    );

    // Run the comparison.
    if let Err(e) = run_comparison(&mut session).await {
        session.errors.push(format!("Comparison failed: {e}"));
    }

    // Restore filter settings.
    if !config.settings.name_filter.is_empty() {
        session.active_filter = Some(config.settings.name_filter[0].clone());
    }
    session.hide_same = !config.settings.show_same;

    Ok(session)
}
