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
//!
//! All `cocomo-lib` and filesystem access in this module returns the typed
//! [`TuiError`], whose variants carry path context; failures surface in the
//! `AlertModal` via [`report_error`]. File I/O uses `fs_err::tokio` so
//! every error names the path it operated on.

use std::{
    future::Future,
    path::{Path, PathBuf},
};

use cocomo_lib::{
    CompareConfig, ContentCache, DirComparison, Endpoint, FileSystem, FsError,
    FsPair, Grammar, LineInfo, NullPrompter, Prompter, Provider,
    SessionConfig, SyncOperation, SyncResult, SyncRules, TextCompareSettings,
    compare_texts, default_session_dir, execute_transfers,
    list_saved_sessions, report::generate_report,
};
use ratatui_kit::crossterm::event::KeyEvent;

use crate::{
    error::{Result, TuiError},
    keys::{cycle_sync_op, handle_session_key},
    session_store::session_to_config,
    state::{
        APP_STATE, AppState, NamePattern, SessionAction, SessionContent,
        SessionMode, SessionView, SettingItem, SyncStage,
    },
};

/// Compare two directories and return the raw result.
///
/// The scan runs to full depth so the dir view can expand directories in
/// place without re-comparing; sync planning is unaffected (it uses its own
/// [`SyncRules`]).
///
/// Takes explicit endpoints instead of a `&mut SessionView` so callers can
/// run it inside a spawned task without holding the `APP_STATE` lock across
/// an `await` — and without constructing a throwaway session just to satisfy
/// the signature. Use [`SessionView::set_comparison`] to put the result
/// into a session. Both sides on the same provider instance compare
/// through the single-filesystem path; a mixed pair compares across
/// providers.
pub(crate) async fn compare(
    left: &Endpoint,
    right: &Endpoint,
    compare_content: bool,
) -> Result<DirComparison> {
    let cache = ContentCache::default_config();

    let config = if compare_content {
        CompareConfig::full()
    } else {
        CompareConfig::meta_data_only()
    };

    if left.same_provider(right) {
        Ok(cocomo_lib::compare_directories_node(
            left.fs(),
            left.path(),
            right.path(),
            &config,
            Some(&cache),
        )
        .await?)
    } else {
        Ok(cocomo_lib::compare_directories_pair_node(
            left.fs(),
            right.fs(),
            left.path(),
            right.path(),
            &config,
            Some(&cache),
        )
        .await?)
    }
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

/// Read a text file for comparison through its endpoint's provider.
///
/// A missing file yields empty content: one side of a comparison can
/// legitimately not exist (an orphan entry), and the diff then shows the
/// other side as entirely added. Any other failure is propagated with the
/// provider's structured error context instead of being swallowed.
async fn read_text_file(fs: &Provider, path: &Path) -> Result<String> {
    match fs.read(path, None).await {
        Ok(bytes) => Ok(String::from_utf8_lossy(&bytes).into_owned()),
        Err(FsError::NotFound { .. }) => Ok(String::new()),
        Err(e) => Err(TuiError::Lib(e)),
    }
}

/// Split file content into line info entries with grammar classification.
///
/// Derives the lines from an already-read string so each file is read
/// exactly once per comparison.
fn line_infos(content: &str, grammar: &Grammar) -> Vec<LineInfo> {
    content
        .lines()
        .enumerate()
        .map(|(i, line)| {
            LineInfo::new(i + 1, line.to_string()).with_grammar(grammar)
        })
        .collect()
}

/// Run a text comparison and create a new [`SessionView`].
async fn run_text_comparison(
    left: Endpoint,
    right: Endpoint,
) -> Result<SessionView> {
    // Detect grammar from the left file (fall back to right if the left
    // file is known to be missing; a remote left side is assumed present
    // since probing it would cost a round trip).
    let grammar = if left.is_local() && !left.path().exists() {
        detect_grammar(right.path())
    } else {
        detect_grammar(left.path())
    };

    // Read each file exactly once and derive both the raw content and the
    // line info from the same string.
    let left_content = read_text_file(left.fs(), left.path()).await?;
    let right_content = read_text_file(right.fs(), right.path()).await?;
    let left_lines = line_infos(&left_content, &grammar);
    let right_lines = line_infos(&right_content, &grammar);

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

/// Plan a sync operation (dry-run) for the given endpoints.
///
/// Takes explicit endpoints instead of a `&mut SessionView` so callers can
/// run it inside a spawned task without holding the `APP_STATE` lock across
/// an `await`.
async fn plan_sync_session(
    left: &Endpoint,
    right: &Endpoint,
    operation: SyncOperation,
    compare_content: bool,
) -> Result<SyncResult> {
    let rules = SyncRules {
        operation,
        dry_run: true,
        max_depth: None,
        compare_content,
    };

    if left.same_provider(right) {
        Ok(
            cocomo_lib::plan_sync(
                left.fs(),
                left.path(),
                right.path(),
                &rules,
            )
            .await?,
        )
    } else {
        Ok(cocomo_lib::plan_sync_pair(
            cocomo_lib::FsPair::Separate(left.fs(), right.fs()),
            left.path(),
            right.path(),
            &rules,
        )
        .await?)
    }
}

/// Execute a sync operation for the given endpoints.
///
/// Like [`plan_sync_session`], takes explicit endpoints so it can run in a
/// spawned task without holding a lock across an `await`.
async fn execute_sync_session(
    left: &Endpoint,
    right: &Endpoint,
    operation: SyncOperation,
    compare_content: bool,
) -> Result<SyncResult> {
    let rules = SyncRules {
        operation,
        dry_run: false,
        max_depth: None,
        compare_content,
    };

    if left.same_provider(right) {
        Ok(cocomo_lib::sync_directories(
            left.fs(),
            left.path(),
            right.path(),
            &rules,
        )
        .await?)
    } else {
        Ok(cocomo_lib::sync_directories_pair(
            cocomo_lib::FsPair::Separate(left.fs(), right.fs()),
            left.path(),
            right.path(),
            &rules,
        )
        .await?)
    }
}

/// Rebuild an endpoint from a saved session's provider reference.
///
/// Local refs resolve without any I/O. Remote refs re-resolve their
/// credentials through `prompter`; the running TUI must pass a
/// [`NullPrompter`] because a terminal prompt would corrupt the alternate
/// screen.
fn resolve_endpoint_ref(
    provider: &str,
    path: PathBuf,
    prompter: &dyn Prompter,
) -> Result<Endpoint> {
    Ok(Endpoint::from_ref(provider, path, prompter)?)
}

/// Create a directory comparison session from a saved config.
async fn create_session_from_config(
    config: &SessionConfig,
) -> Result<SessionView> {
    // Rebuild both endpoints from their provider refs. Remote endpoints
    // re-resolve without prompting: the UI owns the terminal while this
    // runs, so credentials must come from a profile, the environment, or
    // the keychain.
    let prompter = NullPrompter;
    let left = resolve_endpoint_ref(
        &config.left.provider,
        config.left.path.clone(),
        &prompter,
    )?;
    let right = resolve_endpoint_ref(
        &config.right.provider,
        config.right.path.clone(),
        &prompter,
    )?;

    let compare_files = config.settings.compare_files;

    // Run the comparison; propagate failures so the caller can report them
    // instead of opening a session with a permanently empty view.
    let comparison = compare(&left, &right, compare_files).await?;

    let mut session = SessionView::new_dir_compare(left, right, compare_files);
    session.set_comparison(Some(comparison));

    // Restore the display filters.
    if let Some(dir) = session.dir_mut() {
        dir.display.show_same = config.settings.show_same;
        dir.display.show_different = config.settings.show_different;
        dir.display.show_orphans = config.settings.show_orphans;
    }
    // Restore the name filter; a saved pattern that no longer compiles is
    // skipped with a warning instead of failing the whole load.
    if let Some(raw) = config.settings.name_filter.first() {
        match NamePattern::new(raw) {
            Ok(pattern) => session.ui.name_filter = Some(pattern),
            Err(e) => {
                report_error(format!(
                    "Ignoring invalid saved filter `{raw}`: {e}"
                ));
            }
        }
    }

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
        .filter(|s| s.content.is_dir())
        .map(|s| s.core.id)
}

/// Return the stable id of the active session, whatever it compares.
///
/// A transfer can also target the two files of a text-diff session, so it
/// cannot go through [`active_dir_session_id`].
fn active_session_id() -> Option<usize> {
    let state = APP_STATE.state();
    state.read().active().map(|s| s.core.id)
}

/// The session inputs an async task needs, captured under a single read
/// lock so that no guard is ever held across an `await`.
struct SessionSnapshot {
    left: Endpoint,
    right: Endpoint,
    compare_content: bool,
    sync_operation: SyncOperation,
}

impl SessionSnapshot {
    /// Capture the snapshot from a directory session; `None` for a text
    /// session, which has no directory-scoped async work.
    fn from_session(session: &SessionView) -> Option<Self> {
        let dir = session.dir()?;
        Some(Self {
            left: session.core.left.clone(),
            right: session.core.right.clone(),
            compare_content: dir.compare_content,
            sync_operation: dir.sync_operation,
        })
    }
}

/// The inputs of a single transfer, captured under one read lock.
struct TransferSnapshot {
    left: Endpoint,
    right: Endpoint,
    /// Roots the executor joins the entry name onto: the compared
    /// directories for a dir session, the directories *containing* the
    /// compared files for a text session.
    left_root: PathBuf,
    right_root: PathBuf,
    is_dir: bool,
    compare_content: bool,
}

impl TransferSnapshot {
    fn from_session(session: &SessionView) -> Self {
        // A text session's endpoints are the files themselves, while the
        // executor addresses an entry as `root.join(name)`.
        let root = |path: &Path, is_dir: bool| {
            if is_dir {
                path.to_path_buf()
            } else {
                path.parent().unwrap_or(path).to_path_buf()
            }
        };
        let is_dir = session.content.is_dir();
        Self {
            left: session.core.left.clone(),
            right: session.core.right.clone(),
            left_root: root(session.core.left.path(), is_dir),
            right_root: root(session.core.right.path(), is_dir),
            is_dir,
            compare_content: session
                .dir()
                .is_some_and(|dir| dir.compare_content),
        }
    }
}

/// What a transfer task must write back: a fresh comparison for a dir
/// session, fresh diff content for a text session.
enum PostTransfer {
    Dir(DirComparison),
    Text(SessionContent),
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
    Fut: Future<Output = Result<Out>> + Send + 'static,
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
                    let compare_content = app
                        .active()
                        .and_then(|s| s.dir())
                        .is_some_and(|d| d.compare_content);
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
                    let session =
                        app.active().filter(|s| s.content.is_dir())?;
                    let left = session.core.left.parent()?;
                    let right = session.core.right.parent()?;
                    Some((left, right, session.dir()?.compare_content))
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
                move |app| {
                    app.session(id).and_then(SessionSnapshot::from_session)
                },
                move |snapshot| async move {
                    // Clear the comparison so the view shows a loading
                    // state.
                    update_session(id, |s| s.set_comparison(None));

                    compare(
                        &snapshot.left,
                        &snapshot.right,
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
                move |app| {
                    app.session(id).and_then(SessionSnapshot::from_session)
                },
                move |snapshot| async move {
                    // Record the requested operation before planning.
                    update_session(id, |s| {
                        if let Some(dir) = s.dir_mut() {
                            dir.sync_operation = operation;
                        }
                    });

                    plan_sync_session(
                        &snapshot.left,
                        &snapshot.right,
                        operation,
                        snapshot.compare_content,
                    )
                    .await
                },
                move |app, result| {
                    if let Some(s) = app.session_mut(id) {
                        // A non-empty plan is executed only after the user
                        // approves it in the confirmation modal; an empty
                        // plan has nothing to approve, so the preview
                        // alone is shown. The modal is not opened over
                        // another mode the user may have entered while the
                        // plan was running.
                        let confirm = result.planned_count() > 0;
                        s.set_sync(SyncStage::Planned(result));
                        if confirm && matches!(s.ui.mode, SessionMode::Normal)
                        {
                            s.ui.mode = SessionMode::SyncConfirm;
                        }
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
                move |app| {
                    app.session(id).and_then(SessionSnapshot::from_session)
                },
                move |snapshot| async move {
                    execute_sync_session(
                        &snapshot.left,
                        &snapshot.right,
                        snapshot.sync_operation,
                        snapshot.compare_content,
                    )
                    .await
                },
                move |app, result| {
                    if let Some(s) = app.session_mut(id) {
                        s.set_sync(SyncStage::Executed(result));
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
                move |app| app.session(id).and_then(session_to_config),
                move |config| async move {
                    let session_dir = default_session_dir();
                    fs_err::tokio::create_dir_all(&session_dir)
                        .await
                        .map_err(|e| TuiError::io(&session_dir, e))?;
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
                    // browsing; the list lives in the mode.
                    if let Some(s) = app.session_mut(id)
                        && let SessionMode::SessionBrowser { saved_sessions } =
                            &mut s.ui.mode
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
        SessionAction::Transfer { items } => {
            let Some(id) = active_session_id() else {
                return;
            };
            spawn_task(
                move |app| app.session(id).map(TransferSnapshot::from_session),
                move |snapshot| async move {
                    let pair = if snapshot.left.same_provider(&snapshot.right)
                    {
                        FsPair::Shared(snapshot.left.fs())
                    } else {
                        FsPair::Separate(
                            snapshot.left.fs(),
                            snapshot.right.fs(),
                        )
                    };
                    let result = execute_transfers(
                        &items,
                        &snapshot.left_root,
                        &snapshot.right_root,
                        pair,
                    )
                    .await;
                    // Non-fatal per-item failures are collected by the
                    // executor; surface them before re-comparing.
                    if result.failed > 0 {
                        let details: Vec<String> = result
                            .errors
                            .iter()
                            .map(|e| e.to_string())
                            .collect();
                        report_error(format!(
                            "Transfer failed: {}",
                            details.join("; ")
                        ));
                    }
                    // Re-run the comparison so the view reflects the
                    // transfer.
                    if snapshot.is_dir {
                        let comparison = compare(
                            &snapshot.left,
                            &snapshot.right,
                            snapshot.compare_content,
                        )
                        .await?;
                        Ok(PostTransfer::Dir(comparison))
                    } else {
                        let session =
                            run_text_comparison(snapshot.left, snapshot.right)
                                .await?;
                        Ok(PostTransfer::Text(session.content))
                    }
                },
                move |app, post| {
                    let Some(session) = app.session_mut(id) else {
                        return;
                    };
                    match post {
                        PostTransfer::Dir(comparison) => {
                            session.set_comparison(Some(comparison));
                        }
                        PostTransfer::Text(content) => {
                            session.content = content;
                            // The transferred file may have lost lines (or
                            // vanished), so keep the selection in range.
                            let rows = session.row_count();
                            if session
                                .ui
                                .table_state
                                .selected()
                                .is_some_and(|s| s >= rows)
                            {
                                session
                                    .ui
                                    .table_state
                                    .select(rows.checked_sub(1));
                            }
                        }
                    }
                },
                "Comparison after transfer failed",
            );
        }
        SessionAction::GenerateReport {
            format,
            config,
            destination,
        } => {
            let Some(id) = active_dir_session_id() else {
                return;
            };
            spawn_task(
                move |app| app.session(id)?.dir()?.comparison.clone(),
                move |comparison| async move {
                    let report =
                        generate_report(&comparison, format, &config.0);
                    fs_err::tokio::write(&destination, report)
                        .await
                        .map_err(|e| TuiError::io(&destination, e))?;
                    Ok(())
                },
                move |app, ()| {
                    // Close the modal only if it is still open; a failure
                    // never reaches here (it goes to the `AlertModal`
                    // instead, leaving the draft editable).
                    if let Some(s) = app.session_mut(id)
                        && matches!(s.ui.mode, SessionMode::Report { .. })
                    {
                        s.ui.mode = SessionMode::Normal;
                    }
                },
                "Report generation failed",
            );
        }
        SessionAction::None => {}
    }
}

// ---------------------------------------------------------------------------
// Synchronous mutations
// ---------------------------------------------------------------------------

/// Handle a key press for the active directory-comparison session.
///
/// Locks `APP_STATE` briefly so [`handle_session_key`] can mutate the
/// session, releases the lock, then dispatches the resulting intent via
/// [`process_session_action`]. Returns `true` only when the active session
/// has the expected content and the key is bound in its current scope;
/// unbound keys are reported as not consumed so sibling handlers in the
/// same input layer still get a chance.
pub(crate) fn dispatch_dir_key(key: KeyEvent) -> bool {
    dispatch_key(|s| s.content.is_dir(), key)
}

/// Handle a key press for the active text-diff session.
///
/// Like [`dispatch_dir_key`], scoped to text-diff sessions.
pub(crate) fn dispatch_text_key(key: KeyEvent) -> bool {
    dispatch_key(|s| s.content.is_text(), key)
}

/// Handle a key press for the open transfer dialog.
///
/// Like [`dispatch_dir_key`], but scoped to a session whose dialog is open.
/// The dialog is a pure key interface, so `l`/`r`/`b`/`Esc` are its only
/// owned keys; every other key is swallowed by its exclusive input layer.
pub(crate) fn dispatch_dialog_key(key: KeyEvent) -> bool {
    dispatch_key(|s| matches!(s.ui.mode, SessionMode::Transfer { .. }), key)
}

/// Handle a key press for the open filter panel modal.
///
/// Like [`dispatch_dir_key`], scoped to a session whose filter panel is
/// open; the panel's exclusive input layer keeps lower handlers from
/// seeing keys.
pub(crate) fn dispatch_filter_panel_key(key: KeyEvent) -> bool {
    dispatch_key(
        |s| matches!(s.ui.mode, SessionMode::FilterPanel { .. }),
        key,
    )
}

/// Handle a key press for the open session browser modal.
pub(crate) fn dispatch_browser_key(key: KeyEvent) -> bool {
    dispatch_key(
        |s| matches!(s.ui.mode, SessionMode::SessionBrowser { .. }),
        key,
    )
}

/// Handle a key press for the open settings modal.
pub(crate) fn dispatch_settings_key(key: KeyEvent) -> bool {
    dispatch_key(|s| matches!(s.ui.mode, SessionMode::Settings), key)
}

/// Handle a key press for the open report modal.
pub(crate) fn dispatch_report_key(key: KeyEvent) -> bool {
    dispatch_key(|s| matches!(s.ui.mode, SessionMode::Report { .. }), key)
}

/// Approve the planned sync: close the confirmation modal and execute.
///
/// Bound to the modal's confirm action (`y`/`Enter`). Execution re-derives
/// the transfers from the current filesystem state, so the approved plan
/// is a preview rather than a fixed list.
pub(crate) fn confirm_sync() {
    let action = {
        let state = APP_STATE.state();
        let mut app = state.write();
        let Some(session) = app.active_mut().filter(|s| {
            s.content.is_dir() && matches!(s.ui.mode, SessionMode::SyncConfirm)
        }) else {
            return;
        };
        session.ui.mode = SessionMode::Normal;
        SessionAction::ExecuteSync
    };
    process_session_action(action);
}

/// Reject the planned sync: close the confirmation modal, keeping the
/// preview on screen for review.
///
/// Bound to the modal's cancel action (`n`/`N`/`Esc`). From the preview,
/// `Esc` discards it and `S` re-plans and re-confirms.
pub(crate) fn reject_sync() {
    let state = APP_STATE.state();
    let mut app = state.write();
    if let Some(session) = app.active_mut()
        && matches!(session.ui.mode, SessionMode::SyncConfirm)
    {
        session.ui.mode = SessionMode::Normal;
    }
}

fn dispatch_key(
    is_expected: impl Fn(&SessionView) -> bool,
    key: KeyEvent,
) -> bool {
    let action = {
        let state = APP_STATE.state();
        let mut app = state.write();
        let Some(session) = app.active_mut().filter(|s| is_expected(s)) else {
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

/// Update the input buffer of the active session's text-entry mode.
pub(crate) fn set_mode_buffer(value: String) {
    if let Some(session) = APP_STATE.state().write().active_mut() {
        match &mut session.ui.mode {
            SessionMode::SaveSession { name: buf }
            | SessionMode::Search { query: buf }
            | SessionMode::Command { input: buf } => *buf = value,
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
        if !matches!(session.ui.mode, SessionMode::SaveSession { .. }) {
            return false;
        }
        let name = if name.is_empty() {
            session.core.name.clone()
        } else {
            name
        };
        session.ui.mode = SessionMode::Normal;
        name
    };
    process_session_action(SessionAction::SaveSession { name });
    true
}

/// Load the saved session `name` from the session directory and close the
/// browser modal.
///
/// Dispatched by the browser's `Select` on `Enter`.
pub(crate) fn load_saved_session(name: String) {
    let path = {
        let state = APP_STATE.state();
        let mut app = state.write();
        let Some(session) = app.active_mut() else {
            return;
        };
        if !matches!(session.ui.mode, SessionMode::SessionBrowser { .. }) {
            return;
        }
        session.ui.mode = SessionMode::Normal;
        default_session_dir().join(format!("{name}.toml"))
    };
    process_session_action(SessionAction::LoadSession { path });
}

/// Toggle or cycle the settings modal entry `item`.
///
/// Dispatched by the settings modal's `Select` on `Enter`; the modal stays
/// open so several settings can be changed in a row.
pub(crate) fn toggle_setting(item: SettingItem) {
    if let Some(session) = APP_STATE.state().write().active_mut()
        && matches!(session.ui.mode, SessionMode::Settings)
        && let Some(dir) = session.dir_mut()
    {
        match item {
            SettingItem::CompareContent => {
                dir.compare_content = !dir.compare_content;
            }
            SettingItem::HideSame => {
                dir.display.show_same = !dir.display.show_same;
                session.ui.table_state.select(Some(0));
            }
            SettingItem::SyncOperation => {
                dir.sync_operation = cycle_sync_op(&dir.sync_operation, 1);
            }
        }
    }
}

/// Record the status checkboxes of the open filter panel draft.
///
/// Called by the panel's `MultiSelect` whenever a checkbox is toggled; the
/// draft (not the session) changes, so the table behind the modal keeps
/// its current appearance until the draft is applied.
pub(crate) fn set_filter_panel_statuses(
    show_same: bool,
    show_different: bool,
    show_orphans: bool,
) {
    if let Some(session) = APP_STATE.state().write().active_mut()
        && let SessionMode::FilterPanel { draft } = &mut session.ui.mode
    {
        draft.show_same = show_same;
        draft.show_different = show_different;
        draft.show_orphans = show_orphans;
    }
}

/// Record the format chosen in the open report modal draft.
pub(crate) fn set_report_format(format: cocomo_lib::report::ReportFormat) {
    if let Some(session) = APP_STATE.state().write().active_mut()
        && let SessionMode::Report { draft } = &mut session.ui.mode
    {
        draft.format = format;
    }
}

/// Record the inclusion checkboxes of the open report modal draft.
pub(crate) fn set_report_options(
    include_same: bool,
    include_different: bool,
    include_orphans: bool,
    include_subdirectories: bool,
    include_file_details: bool,
) {
    if let Some(session) = APP_STATE.state().write().active_mut()
        && let SessionMode::Report { draft } = &mut session.ui.mode
    {
        draft.include_same = include_same;
        draft.include_different = include_different;
        draft.include_orphans = include_orphans;
        draft.include_subdirectories = include_subdirectories;
        draft.include_file_details = include_file_details;
    }
}

/// Apply the submitted search query: jump to the next matching row (from
/// just after the current selection, wrapping) and return to normal mode.
///
/// Returns `true` when the session was in search mode (so the input
/// component may close its editing layer), `false` otherwise.
pub(crate) fn submit_search(query: String) -> bool {
    let state = APP_STATE.state();
    let mut app = state.write();
    let Some(session) = app.active_mut() else {
        return false;
    };
    if !matches!(session.ui.mode, SessionMode::Search { .. }) {
        return false;
    }
    let from = session.ui.table_state.selected().unwrap_or(0);
    if let Some(row) = session.find_row_matching(&query, from) {
        session.ui.table_state.select(Some(row));
    }
    session.ui.mode = SessionMode::Normal;
    true
}

/// Run the submitted command-line command and return to normal mode.
///
/// The BCS script engine is deferred to v2 (requirements §16), so command
/// mode executes a small set of built-in commands that map onto existing
/// session actions. Unknown commands surface in the `AlertModal`.
///
/// Returns `true` when the session was in command mode, `false` otherwise.
pub(crate) fn submit_command(input: String) -> bool {
    let (line, sync_operation) = {
        let state = APP_STATE.state();
        let mut app = state.write();
        let Some(session) = app.active_mut() else {
            return false;
        };
        if !matches!(session.ui.mode, SessionMode::Command { .. }) {
            return false;
        }
        let operation = session.dir().map(|d| d.sync_operation);
        session.ui.mode = SessionMode::Normal;
        (input.trim().to_string(), operation)
    };
    if line.is_empty() {
        return true;
    }
    let (name, arg) = line
        .split_once(char::is_whitespace)
        .unwrap_or((line.as_str(), ""));
    match name.to_lowercase().as_str() {
        "reload" => process_session_action(SessionAction::Reload),
        "sync" => {
            if let Some(operation) = sync_operation {
                process_session_action(SessionAction::PlanSync { operation });
            }
        }
        "filter" => {
            let pattern = arg.trim();
            let compiled = if pattern.is_empty() {
                None
            } else {
                match NamePattern::new(pattern) {
                    Ok(p) => Some(p),
                    Err(e) => {
                        report_error(format!(
                            "Invalid filter pattern `{pattern}`: {e}"
                        ));
                        return true;
                    }
                }
            };
            if let Some(session) = APP_STATE.state().write().active_mut() {
                session.ui.name_filter = compiled;
                session.ui.table_state.select(Some(0));
            }
        }
        "help" => toggle_help(),
        _ => report_error(format!(
            "Unknown command `{name}`. Available: reload, sync, filter \
             <pattern>, help"
        )),
    }
    true
}

/// Switch the active session into `mode`.
pub(crate) fn set_active_mode(mode: SessionMode) {
    if let Some(session) = APP_STATE.state().write().active_mut() {
        session.ui.mode = mode;
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

/// Toggle the bottom help bar.
pub(crate) fn toggle_help() {
    let state = APP_STATE.state();
    let mut app = state.write();
    app.show_help = !app.show_help;
}

/// Dismiss the pending top-level error message.
pub(crate) fn dismiss_error() {
    APP_STATE.state().write().error_message = None;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn read_text_file_returns_content() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        fs_err::tokio::write(&path, "hello\nworld\n").await.unwrap();
        let content = read_text_file(Endpoint::local("/").fs(), &path)
            .await
            .unwrap();
        assert_eq!(content, "hello\nworld\n");
    }

    #[tokio::test]
    async fn read_text_file_treats_missing_file_as_empty() {
        // One side of a comparison may legitimately not exist (orphan
        // entry); that must not fail the comparison.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing.txt");
        let content = read_text_file(Endpoint::local("/").fs(), &path)
            .await
            .unwrap();
        assert!(content.is_empty());
    }

    #[tokio::test]
    async fn read_text_file_propagates_other_io_errors() {
        // Reading a directory is not a NotFound failure, so the error must
        // be propagated instead of swallowed.
        let dir = tempfile::tempdir().unwrap();
        let err = read_text_file(Endpoint::local("/").fs(), dir.path())
            .await
            .unwrap_err();
        assert!(matches!(err, TuiError::Lib(_)));
    }

    #[test]
    fn line_infos_are_one_based_and_complete() {
        let grammar = Grammar::plain_text();
        let lines = line_infos("alpha\nbeta", &grammar);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].number, 1);
        assert_eq!(lines[0].content, "alpha");
        assert_eq!(lines[1].number, 2);
        assert_eq!(lines[1].content, "beta");
    }

    #[tokio::test]
    async fn run_text_comparison_reads_missing_left_as_empty() {
        let dir = tempfile::tempdir().unwrap();
        let left = Endpoint::local(dir.path().join("gone.rs"));
        let right = Endpoint::local(dir.path().join("here.rs"));
        fs_err::tokio::write(right.path(), "fn a() {}\n")
            .await
            .unwrap();
        let session = run_text_comparison(left, right).await.unwrap();
        let text = session.text().expect("a text session was created");
        assert!(text.left_lines.is_empty());
        assert_eq!(text.right_lines.len(), 1);
        assert_eq!(text.right_lines[0].content, "fn a() {}");
        assert!(text.text_diff.is_some());
    }
}
