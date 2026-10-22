// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Controller: the single owner of all `APP_STATE` mutations.
//!
//! Components never write to the global atom directly; they call the
//! functions in this module. Async work goes through
//! [`process_session_action`], which turns [`SessionAction`] intents from
//! the key handlers into spawned tasks. Every task follows the same lock
//! discipline — extract inputs under a short-lived read lock, run the async
//! operation outside any lock, write the result back under a fresh write
//! lock (targeting the originating session by id) — implemented once in
//! [`spawn_task`].

use std::{
    future::Future,
    path::{Path, PathBuf},
};

use cocomo_lib::{
    CompareConfig, ContentCache, DirComparison, Grammar, LineInfo, LocalFs,
    SessionConfig, SyncOperation, SyncResult, SyncRules, TextCompareSettings,
    compare_texts,
};
use ratatui_kit::crossterm::event::KeyEvent;

use crate::{
    keys::handle_session_key,
    session_store::{
        default_session_dir, list_saved_sessions, session_to_config,
    },
    state::{
        APP_STATE, AppState, SessionAction, SessionMode, SessionType,
        SessionView,
    },
};

/// Compare two directories and return the raw result.
///
/// Takes explicit paths instead of a `&mut SessionView` so callers can run
/// it inside a spawned task without holding the `APP_STATE` lock across an
/// `await` — and without constructing a throwaway session just to satisfy
/// the signature. Use [`SessionView::set_comparison`] to put the result
/// into a session.
pub(crate) async fn compare(
    left: &Path,
    right: &Path,
    compare_content: bool,
) -> anyhow::Result<DirComparison> {
    let fs = LocalFs::new("local");
    let cache = ContentCache::default_config();

    let mut config = if compare_content {
        CompareConfig::full()
    } else {
        CompareConfig::meta_data_only()
    };
    config.max_depth = Some(1);

    Ok(cocomo_lib::compare_directories_node(
        &fs,
        left,
        right,
        &config,
        Some(&cache),
    )
    .await?)
}

/// Detect a suitable grammar for syntax classification based on file
/// extension.
fn detect_grammar(path: &Path) -> Grammar {
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
async fn run_text_comparison(
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

/// Plan a sync operation (dry-run) for the given directories.
///
/// Takes explicit paths instead of a `&mut SessionView` so callers can run
/// it inside a spawned task without holding the `APP_STATE` lock across an
/// `await`.
async fn plan_sync_session(
    left: &Path,
    right: &Path,
    operation: SyncOperation,
    compare_content: bool,
) -> anyhow::Result<SyncResult> {
    let fs = LocalFs::new("local");
    let rules = SyncRules {
        operation,
        dry_run: true,
        max_depth: None,
        compare_content,
    };

    Ok(cocomo_lib::plan_sync(&fs, left, right, &rules).await?)
}

/// Execute a sync operation for the given directories.
///
/// Like [`plan_sync_session`], takes explicit paths so it can run in a
/// spawned task without holding a lock across an `await`.
async fn execute_sync_session(
    left: &Path,
    right: &Path,
    operation: SyncOperation,
    compare_content: bool,
) -> anyhow::Result<SyncResult> {
    let fs = LocalFs::new("local");
    let rules = SyncRules {
        operation,
        dry_run: false,
        max_depth: None,
        compare_content,
    };

    Ok(cocomo_lib::sync_directories(&fs, left, right, &rules).await?)
}

/// Create a directory comparison session from a saved config.
async fn create_session_from_config(
    config: &SessionConfig,
) -> anyhow::Result<SessionView> {
    // Resolve the paths from the provider ref.
    let left_path = &config.left.path;
    let right_path = &config.right.path;

    let compare_files = config.settings.compare_files;

    // Run the comparison; propagate failures so the caller can report them
    // instead of opening a session with a permanently empty view.
    let comparison = compare(left_path, right_path, compare_files).await?;

    let mut session = SessionView::new_dir_compare(
        left_path.clone(),
        right_path.clone(),
        compare_files,
    );
    session.set_comparison(Some(comparison));

    // Restore filter settings.
    if !config.settings.name_filter.is_empty() {
        session.active_filter = Some(config.settings.name_filter[0].clone());
    }
    session.hide_same = !config.settings.show_same;

    Ok(session)
}

/// Report a failure from a spawned async task via the global `AlertModal`.
fn report_error(msg: impl Into<String>) {
    APP_STATE.state().write().error_message = Some(msg.into());
}

/// Return the stable id of the active directory-comparison session, if any.
///
/// Spawned tasks capture this at spawn time so their results are written
/// back to the originating session even if the user switches tabs while the
/// task is running.
fn active_dir_session_id() -> Option<usize> {
    let state = APP_STATE.state();
    state
        .read()
        .active()
        .filter(|s| s.session_type == SessionType::DirCompare)
        .map(|s| s.id)
}

/// The session inputs an async task needs, captured under a single read
/// lock so that no guard is ever held across an `await`.
struct SessionSnapshot {
    left_path: PathBuf,
    right_path: PathBuf,
    compare_content: bool,
    sync_operation: SyncOperation,
}

impl From<&SessionView> for SessionSnapshot {
    fn from(session: &SessionView) -> Self {
        Self {
            left_path: session.left_path.clone(),
            right_path: session.right_path.clone(),
            compare_content: session.compare_content,
            sync_operation: session.sync_operation,
        }
    }
}

/// Apply a synchronous mutation to the session with the given id under a
/// brief write lock.
///
/// For state updates that must happen between input extraction and the
/// async work (e.g. showing a loading state, recording the requested sync
/// operation). The closure must not await.
fn update_session(id: usize, f: impl FnOnce(&mut SessionView)) {
    if let Some(session) = APP_STATE.state().write().session_mut(id) {
        f(session);
    }
}

/// Spawn an async task that follows the controller's lock discipline:
///
/// 1. `extract` reads the task inputs from [`AppState`] under a short-lived
///    read lock; returning `None` silently aborts the task.
/// 2. `work` runs outside any lock.
/// 3. `apply` writes the result back under a fresh write lock.
///
/// An `Err` from `work` is reported via [`report_error`], prefixed with
/// `err_context`. This is the only place where the "never hold a guard
/// across an `await`" invariant needs to be satisfied.
fn spawn_task<In, Out, Fut, Extract, Work, Apply>(
    extract: Extract,
    work: Work,
    apply: Apply,
    err_context: &'static str,
) where
    In: Send + 'static,
    Out: Send + 'static,
    Fut: Future<Output = anyhow::Result<Out>> + Send + 'static,
    Extract: FnOnce(&AppState) -> Option<In> + Send + 'static,
    Work: FnOnce(In) -> Fut + Send + 'static,
    Apply: FnOnce(&mut AppState, Out) + Send + 'static,
{
    tokio::spawn(async move {
        let input = {
            let state = APP_STATE.state();
            let app = state.read();
            match extract(&app) {
                Some(input) => input,
                None => return,
            }
        };

        match work(input).await {
            Ok(out) => {
                let state = APP_STATE.state();
                apply(&mut state.write(), out);
            }
            Err(e) => report_error(format!("{err_context}: {e}")),
        }
    });
}

/// Process a [`SessionAction`] returned by the key handlers.
///
/// Every arm is a [`spawn_task`] call: inputs are extracted by session id,
/// the `cocomo-lib` call runs outside any lock, and the result is applied
/// to the originating session. Writes to the atom wake the subscribed
/// components, so the UI re-renders on completion.
pub(crate) fn process_session_action(action: SessionAction) {
    match action {
        SessionAction::OpenDir { left, right } => {
            spawn_task(
                move |app| {
                    let compare_content =
                        app.active().is_some_and(|s| s.compare_content);
                    Some((left, right, compare_content))
                },
                move |(left, right, compare_content)| async move {
                    let comparison =
                        compare(&left, &right, compare_content).await?;
                    let mut session = SessionView::new_dir_compare(
                        left,
                        right,
                        compare_content,
                    );
                    session.set_comparison(Some(comparison));
                    Ok(session)
                },
                |app, session| app.add_session(session),
                "Comparison failed",
            );
        }
        SessionAction::OpenFile { left, right } => {
            spawn_task(
                |_| Some(()),
                move |_| async move { run_text_comparison(left, right).await },
                |app, session| app.add_session(session),
                "Text comparison failed",
            );
        }
        SessionAction::GoUp => {
            spawn_task(
                |app| {
                    let session = app.active().filter(|s| {
                        s.session_type == SessionType::DirCompare
                    })?;
                    let left =
                        session.left_path.parent().map(PathBuf::from)?;
                    let right =
                        session.right_path.parent().map(PathBuf::from)?;
                    Some((left, right, session.compare_content))
                },
                move |(left, right, compare_content)| async move {
                    let comparison =
                        compare(&left, &right, compare_content).await?;
                    let mut session = SessionView::new_dir_compare(
                        left,
                        right,
                        compare_content,
                    );
                    session.set_comparison(Some(comparison));
                    Ok(session)
                },
                |app, session| app.add_session(session),
                "Comparison failed",
            );
        }
        SessionAction::Reload => {
            let Some(id) = active_dir_session_id() else {
                return;
            };
            spawn_task(
                move |app| app.session(id).map(SessionSnapshot::from),
                move |snapshot| async move {
                    // Clear the comparison so the view shows a loading
                    // state.
                    update_session(id, |s| s.set_comparison(None));

                    compare(
                        &snapshot.left_path,
                        &snapshot.right_path,
                        snapshot.compare_content,
                    )
                    .await
                },
                move |app, comparison| {
                    if let Some(s) = app.session_mut(id) {
                        s.set_comparison(Some(comparison));
                    }
                },
                "Comparison failed",
            );
        }
        SessionAction::PlanSync { operation } => {
            let Some(id) = active_dir_session_id() else {
                return;
            };
            spawn_task(
                move |app| app.session(id).map(SessionSnapshot::from),
                move |snapshot| async move {
                    // Record the requested operation before planning.
                    update_session(id, |s| s.sync_operation = operation);

                    plan_sync_session(
                        &snapshot.left_path,
                        &snapshot.right_path,
                        operation,
                        snapshot.compare_content,
                    )
                    .await
                },
                move |app, result| {
                    if let Some(s) = app.session_mut(id) {
                        s.sync_planned = Some(result);
                        s.table_state.select(Some(0));
                    }
                },
                "Sync planning failed",
            );
        }
        SessionAction::ExecuteSync => {
            let Some(id) = active_dir_session_id() else {
                return;
            };
            spawn_task(
                move |app| app.session(id).map(SessionSnapshot::from),
                move |snapshot| async move {
                    execute_sync_session(
                        &snapshot.left_path,
                        &snapshot.right_path,
                        snapshot.sync_operation,
                        snapshot.compare_content,
                    )
                    .await
                },
                move |app, result| {
                    if let Some(s) = app.session_mut(id) {
                        s.sync_planned = Some(result);
                        s.table_state.select(Some(0));
                    }
                },
                "Sync execution failed",
            );
        }
        SessionAction::SaveSession { name } => {
            let Some(id) = active_dir_session_id() else {
                return;
            };
            spawn_task(
                move |app| app.session(id).map(session_to_config),
                move |config| async move {
                    let session_dir = default_session_dir();
                    std::fs::create_dir_all(&session_dir)?;
                    let path = session_dir.join(format!("{name}.toml"));
                    config.save_to_file(&path).await?;
                    Ok(())
                },
                |_app, ()| {},
                "Failed to save session",
            );
        }
        SessionAction::ListSessions => {
            let Some(id) = active_dir_session_id() else {
                return;
            };
            spawn_task(
                |_| Some(()),
                |_| async move {
                    let sessions = list_saved_sessions().await?;
                    Ok(sessions
                        .iter()
                        .map(|p| {
                            p.file_stem()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .to_string()
                        })
                        .collect::<Vec<_>>())
                },
                move |app, names| {
                    // Only apply the listing while the session is still
                    // in load mode; the buffer lives in the mode.
                    if let Some(s) = app.session_mut(id)
                        && let SessionMode::LoadSession {
                            saved_sessions, ..
                        } = &mut s.mode
                    {
                        *saved_sessions = names;
                    }
                },
                "Failed to list saved sessions",
            );
        }
        SessionAction::LoadSession { path } => {
            spawn_task(
                |_| Some(()),
                move |_| async move {
                    let config = SessionConfig::load_from_file(&path).await?;
                    create_session_from_config(&config).await
                },
                |app, session| app.add_session(session),
                "Session load failed",
            );
        }
        SessionAction::None => {}
    }
}

// ---------------------------------------------------------------------------
// Synchronous mutations
// ---------------------------------------------------------------------------

/// Handle a key press for the active session of type `expected`.
///
/// Locks `APP_STATE` briefly so [`handle_session_key`] can mutate the
/// session, releases the lock, then dispatches the resulting intent via
/// [`process_session_action`]. Returns `true` only when the key is bound
/// in the session's current scope; unbound keys are reported as not
/// consumed so sibling handlers in the same input layer still get a
/// chance.
pub(crate) fn dispatch_session_key(
    expected: SessionType,
    key: KeyEvent,
) -> bool {
    let action = {
        let state = APP_STATE.state();
        let mut app = state.write();
        let Some(session) =
            app.active_mut().filter(|s| s.session_type == expected)
        else {
            return false;
        };

        handle_session_key(session, key)
    };

    match action {
        Some(action) => {
            process_session_action(action);
            true
        }
        None => false,
    }
}

/// Set the filter input buffer of the active session.
pub(crate) fn set_filter_input(value: String) {
    if let Some(session) = APP_STATE.state().write().active_mut() {
        session.filter_input = value;
    }
}

/// Update the name buffer of the active session's save/load mode.
pub(crate) fn set_mode_name(name: String) {
    if let Some(session) = APP_STATE.state().write().active_mut() {
        match &mut session.mode {
            SessionMode::SaveSession { name: buf }
            | SessionMode::LoadSession { name: buf, .. } => *buf = name,
            _ => {}
        }
    }
}

/// Submit the typed name for saving the active session.
///
/// An empty name falls back to the session's tab title. Returns `true`
/// when the save was dispatched (so the input component may close its
/// editing layer), `false` when the session was not in save mode.
pub(crate) fn submit_save_name(name: String) -> bool {
    let name = {
        let state = APP_STATE.state();
        let mut app = state.write();
        let Some(session) = app.active_mut() else {
            return false;
        };
        if !matches!(session.mode, SessionMode::SaveSession { .. }) {
            return false;
        }
        let name = if name.is_empty() {
            session.name.clone()
        } else {
            name
        };
        session.mode = SessionMode::Normal;
        name
    };
    process_session_action(SessionAction::SaveSession { name });
    true
}

/// Submit the typed name for loading a saved session.
///
/// Returns `true` when the load was dispatched, `false` when the session
/// was not in load mode or the name is still empty (keeping the input
/// open for the user to type one).
pub(crate) fn submit_load_name(name: String) -> bool {
    if name.is_empty() {
        return false;
    }
    let path = {
        let state = APP_STATE.state();
        let mut app = state.write();
        let Some(session) = app.active_mut() else {
            return false;
        };
        if !matches!(session.mode, SessionMode::LoadSession { .. }) {
            return false;
        }
        session.mode = SessionMode::Normal;
        default_session_dir().join(format!("{name}.toml"))
    };
    process_session_action(SessionAction::LoadSession { path });
    true
}

/// Apply `value` as the active session's filter and return to normal mode.
pub(crate) fn submit_filter(value: String) {
    if let Some(session) = APP_STATE.state().write().active_mut() {
        session.active_filter = Some(value);
        session.mode = SessionMode::Normal;
    }
}

/// Switch the active session into `mode`.
pub(crate) fn set_active_mode(mode: SessionMode) {
    if let Some(session) = APP_STATE.state().write().active_mut() {
        session.mode = mode;
    }
}

/// Close the active tab.
///
/// Returns `true` when no sessions remain, meaning the caller should exit.
pub(crate) fn close_active_tab() -> bool {
    let state = APP_STATE.state();
    let mut app = state.write();
    app.close_active();
    app.sessions.is_empty()
}

/// Switch to the next or previous tab (positive `delta` moves forward).
pub(crate) fn switch_tab(delta: isize) {
    APP_STATE.state().write().switch_tab(delta);
}

/// Select the menu item at `index`.
pub(crate) fn set_menu_selection(index: usize) {
    APP_STATE.state().write().menu_selection = index;
}

/// Dismiss the pending top-level error message.
pub(crate) fn dismiss_error() {
    APP_STATE.state().write().error_message = None;
}
