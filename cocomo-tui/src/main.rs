// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! cocomo_tui — Terminal UI for directory comparison.
//!
//! Provides a multi-tab folder comparison view with navigation, status
//! indicators, filtering, and session-based directory exploration. Each tab
//! is an independent comparison session; pressing Enter on a directory opens
//! a new tab comparing the subdirectories.

mod app;
mod session_view;

use std::{path::PathBuf, sync::Arc};

use anyhow::Result;
use app::{
    APP_STATE, AppMode, Focus, SessionAction, SessionType, SessionView, create_session_from_config,
    list_saved_sessions, run_comparison, run_text_comparison, session_to_config,
};
use cocomo_lib::{DirEntry, LocalFs, SessionConfig, SyncRules, TransferAction, TransferItem};
use clap::Parser;
use ratatui_kit::{
    crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    prelude::*,
    ratatui::{
        Frame,
        layout::{Alignment, Constraint, Direction, Rect},
        prelude::Stylize,
        style::{Color, Modifier, Style},
        symbols,
        text::{Line, Span},
        widgets::{Block, Borders, Paragraph, Tabs},
    },
};
use session_view::handle_session_key;

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

/// cocomo — COmpare, COpy & MOve directories and files.
#[derive(Parser, Debug)]
#[command(name = "cocomo-tui", version, about)]
struct Cli {
    /// Left directory path.
    #[arg(short, long)]
    left: Option<PathBuf>,

    /// Right directory path.
    #[arg(short, long)]
    right: Option<PathBuf>,

    /// Compare file content (using hashing).
    #[arg(short = 'c', long, default_value_t = false)]
    content: bool,
}

// ---------------------------------------------------------------------------
// Global key handling
// ---------------------------------------------------------------------------

/// Result of dispatching a global key event.
enum GlobalKeyResult {
    /// The key was not consumed; dispatch to the active session.
    Ignored,
    /// The key was consumed; continue the event loop.
    Consumed,
    /// The key was consumed and the application should quit.
    Quit,
}

/// Handle keys that affect the app globally (tab switching, menu, quit).
///
/// Returns `GlobalKeyResult::Quit` when the application should exit (e.g. on
/// `q` or when closing the last tab), `Consumed` when the key was handled, or
/// `Ignored` to let the session handler take over.
fn handle_global_key(app: &mut app::App, key: KeyEvent) -> GlobalKeyResult {
    const MENU_COUNT: usize = 6; // Sessions, Compare, Sync, View, Filter, Help

    // Ctrl+W — close active tab.
    if key.code == KeyCode::Char('w')
        && key.modifiers.contains(KeyModifiers::CONTROL)
    {
        app.close_active();
        return if app.sessions.is_empty() {
            GlobalKeyResult::Quit
        } else {
            GlobalKeyResult::Consumed
        };
    }

    // Ctrl+Tab — next tab.
    if key.code == KeyCode::Tab
        && key.modifiers.contains(KeyModifiers::CONTROL)
    {
        let is_shift = key.modifiers.contains(KeyModifiers::SHIFT);
        app.switch_tab(if is_shift { -1 } else { 1 });
        return GlobalKeyResult::Consumed;
    }

    // Left/Right arrow keys — navigate menu bar.
    if key.modifiers == KeyModifiers::NONE {
        match key.code {
            KeyCode::Left => {
                app.menu_selection = if app.menu_selection > 0 {
                    app.menu_selection - 1
                } else {
                    MENU_COUNT - 1
                };
                return GlobalKeyResult::Consumed;
            }
            KeyCode::Right => {
                app.menu_selection = (app.menu_selection + 1) % MENU_COUNT;
                return GlobalKeyResult::Consumed;
            }
            _ => {}
        }
    }

    // q — quit (only when not in a session filter mode).
    if key.code == KeyCode::Char('q') || key.code == KeyCode::Char('Q') {
        // If there's an active session in filter mode, let the session handle
        // it.
        if let Some(session) = app.active() {
            if session.mode != AppMode::Filter {
                return GlobalKeyResult::Quit;
            }
        } else {
            return GlobalKeyResult::Quit;
        }
    }

    GlobalKeyResult::Ignored
}

// ---------------------------------------------------------------------------
// Content rendering (used by SessionContent component for text compare & welcome)
// ---------------------------------------------------------------------------

/// Render the welcome screen when no sessions are open.
fn render_welcome(frame: &mut Frame, area: Rect) {
    let lines = vec![
        Line::from("cocomo — COmpare, COpy & MOve"),
        Line::from(""),
        Line::from("Provide --left and --right to start a comparison."),
        Line::from(""),
        Line::from("Press q to quit."),
    ];

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Gray));
    let paragraph = Paragraph::new(lines)
        .block(block)
        .alignment(Alignment::Center);
    frame.render_widget(paragraph, area);
}

/// Render the main content area: either the active session or the welcome
/// screen, within the given area.
fn render_content(frame: &mut Frame, area: Rect, app: &app::App) {
    if let Some(session) = app.active() {
        match session.session_type {
            SessionType::DirCompare => {
                session_view::render_session(frame, area, session);
            }
            SessionType::TextCompare => {
                session_view::render_text_session(frame, area, session);
            }
        }
    } else {
        render_welcome(frame, area);
    }
}

// ---------------------------------------------------------------------------
// SessionContent component (manual Component impl for Frame-based rendering)
// ---------------------------------------------------------------------------

/// Props for [`SessionContent`] — no props; state is read from the global
/// atom.
#[derive(Default, Props)]
struct SessionContentProps;

/// Component that renders the main content area using the existing
/// Frame-based render functions.
///
/// This component reads from `APP_STATE` via `use_atom` in its `update()`
/// method, then renders using `ComponentDrawer`'s frame in `draw()`. This
/// bridges the legacy Frame-based rendering with ratatui-kit's component
/// model.
struct SessionContent {
    app_handle: Option<AtomState<app::App>>,
}

impl Component for SessionContent {
    type Props<'a> = SessionContentProps;

    fn new(_props: &Self::Props<'_>) -> Self {
        Self { app_handle: None }
    }

    fn update(
        &mut self,
        _props: &mut Self::Props<'_>,
        mut hooks: Hooks,
        updater: &mut ComponentUpdater,
    ) {
        let mut hooks =
            hooks.with_context_stack(updater.component_context_stack());
        self.app_handle = Some(hooks.use_atom(&APP_STATE));
    }

    fn draw(&mut self, drawer: &mut ComponentDrawer<'_, '_>) {
        if let Some(handle) = &self.app_handle {
            let app = handle.read();
            render_content(drawer.frame, drawer.area, &app);
        }
    }
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Menu bar item labels, in order: Sessions, Compare, Sync, View, Filter,
/// Help.
const MENU_TITLES: [&str; 6] =
    ["Sessions", "Compare", "Sync", "View", "Filter", "Help"];

// ---------------------------------------------------------------------------
// MenuBar component
// ---------------------------------------------------------------------------

/// Props for [`MenuBar`].
#[derive(Default, Props)]
struct MenuBarProps {
    /// Titles of the menu items.
    menu_titles: Vec<Line<'static>>,
    /// Index of the currently selected menu item.
    selected: usize,
}

/// Top menu bar with tab-like navigation items (Sessions, Compare, Sync,
/// View, Filter, Help).
#[component]
fn MenuBar(props: &MenuBarProps) -> impl Into<AnyElement<'static>> {
    let menu_tabs = Tabs::new(props.menu_titles.clone())
        .select(props.selected)
        .highlight_style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )
        .divider(Span::raw(" | "))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Gray)),
        );

    element!(
        View(height: Constraint::Length(3)) {
            widget(menu_tabs)
        }
    )
}

// ---------------------------------------------------------------------------
// TabBar component
// ---------------------------------------------------------------------------

/// Props for [`TabBar`].
#[derive(Default, Props)]
struct TabBarProps {
    /// Titles of the session tabs.
    tab_titles: Vec<Line<'static>>,
    /// Index of the active session tab.
    active_index: usize,
}

/// Session tab bar showing open comparison sessions. When no sessions are
/// open, an empty bordered block is rendered.
#[component]
fn TabBar(props: &TabBarProps) -> impl Into<AnyElement<'static>> {
    let tab_tabs = if !props.tab_titles.is_empty() {
        Tabs::new(props.tab_titles.clone())
            .select(props.active_index)
            .highlight_style(
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )
            .divider(Span::raw(" | "))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Gray)),
            )
    } else {
        Tabs::new(Vec::<Line>::new()).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Gray)),
        )
    };

    element!(
        View(height: Constraint::Length(3)) {
            widget(tab_tabs)
        }
    )
}

// ---------------------------------------------------------------------------
// Session action processing (async operations triggered by key events)
// ---------------------------------------------------------------------------

/// Process a [`SessionAction`] returned by `handle_session_key`.
///
/// Actions that require async work are spawned as `tokio::spawn` tasks
/// that operate on `APP_STATE`.
fn process_session_action(action: SessionAction) {
    match action {
        SessionAction::OpenDir { left, right } => {
            tokio::spawn(async move {
                let compare_content = APP_STATE
                    .state()
                    .read()
                    .active()
                    .map(|s| s.compare_content)
                    .unwrap_or(false);
                let mut session = SessionView::new_dir_compare(left, right, compare_content);
                if let Err(e) = run_comparison(&mut session).await {
                    session.errors.push(format!("Comparison failed: {e}"));
                }
                APP_STATE.state().write().add_session(session);
            });
        }
        SessionAction::OpenFile { left, right } => {
            tokio::spawn(async move {
                match run_text_comparison(left, right).await {
                    Ok(session) => APP_STATE.state().write().add_session(session),
                    Err(e) => {
                        let state = APP_STATE.state();
                        let mut app = state.write();
                        if let Some(s) = app.active_mut() {
                            s.errors.push(format!("Text comparison failed: {e}"));
                        }
                    }
                }
            });
        }
        SessionAction::GoUp => {
            tokio::spawn(async move {
                let (left_path, right_path, compare_content) = {
                    let state = APP_STATE.state();
                    let app = state.read();
                    match app.active() {
                        Some(s)
                            if s.session_type == SessionType::DirCompare => (
                            s.left_path.clone(),
                            s.right_path.clone(),
                            s.compare_content,
                        ),
                        _ => return,
                    }
                };

                let left_parent = left_path.parent().map(PathBuf::from);
                let right_parent = right_path.parent().map(PathBuf::from);

                if let (Some(left), Some(right)) = (left_parent, right_parent) {
                    let mut session = SessionView::new_dir_compare(left, right, compare_content);
                    if let Err(e) = run_comparison(&mut session).await {
                        session.errors.push(format!("Comparison failed: {e}"));
                    }
                    APP_STATE.state().write().add_session(session);
                }
            });
        }
        SessionAction::Reload => {
            tokio::spawn(async move {
                let (left_path, right_path, compare_content) = {
                    let state = APP_STATE.state();
                    let app = state.read();
                    match app.active() {
                        Some(s)
                            if s.session_type == SessionType::DirCompare => (
                            s.left_path.clone(),
                            s.right_path.clone(),
                            s.compare_content,
                        ),
                        _ => return,
                    }
                };

                APP_STATE.state().write().active_mut().map(|s| {
                    s.comparison = None;
                });

                let mut session = SessionView::new_dir_compare(left_path, right_path, compare_content);
                if let Err(e) = run_comparison(&mut session).await {
                    session.errors.push(format!("Comparison failed: {e}"));
                }

                APP_STATE.state().write().active_mut().map(|s| {
                    s.comparison = session.comparison;
                    s.table_state.select(Some(0));
                });
            });
        }
        SessionAction::PlanSync { operation } => {
            tokio::spawn(async move {
                // Extract session data under read lock, then release before async work.
                let (left_path, right_path, compare_content) = {
                    let state = APP_STATE.state();
                    let app = state.read();
                    match app.active() {
                        Some(s) if s.session_type == SessionType::DirCompare => (
                            s.left_path.clone(),
                            s.right_path.clone(),
                            s.compare_content,
                        ),
                        _ => return,
                    }
                };

                // Set the sync operation under write lock (brief, no await).
                APP_STATE.state().write().active_mut().map(|s| {
                    s.sync_operation = operation;
                });

                // Run sync planning outside any lock.
                let fs = LocalFs::new("local");
                let rules = SyncRules {
                    operation,
                    dry_run: true,
                    max_depth: None,
                    compare_content,
                };

                match cocomo_lib::plan_sync(&fs, &left_path, &right_path, &rules).await {
                    Ok(result) => {
                        APP_STATE.state().write().active_mut().map(|s| {
                            s.sync_planned = Some(result);
                            s.table_state.select(Some(0));
                        });
                    }
                    Err(e) => {
                        let state = APP_STATE.state();
                        let mut app = state.write();
                        if let Some(s) = app.active_mut() {
                            s.errors.push(format!("Sync planning failed: {e}"));
                        }
                    }
                }
            });
        }
        SessionAction::ExecuteSync => {
            tokio::spawn(async move {
                // Extract session data under read lock, then release before async work.
                let (left_path, right_path, compare_content, operation) = {
                    let state = APP_STATE.state();
                    let app = state.read();
                    match app.active() {
                        Some(s) if s.session_type == SessionType::DirCompare => (
                            s.left_path.clone(),
                            s.right_path.clone(),
                            s.compare_content,
                            s.sync_operation,
                        ),
                        _ => return,
                    }
                };

                // Run sync execution outside any lock.
                let fs = LocalFs::new("local");
                let rules = SyncRules {
                    operation,
                    dry_run: false,
                    max_depth: None,
                    compare_content,
                };

                match cocomo_lib::sync_directories(&fs, &left_path, &right_path, &rules).await {
                    Ok(result) => {
                        APP_STATE.state().write().active_mut().map(|s| {
                            s.sync_planned = Some(result);
                            s.table_state.select(Some(0));
                        });
                    }
                    Err(e) => {
                        let state = APP_STATE.state();
                        let mut app = state.write();
                        if let Some(s) = app.active_mut() {
                            s.errors.push(format!("Sync execution failed: {e}"));
                        }
                    }
                }
            });
        }
        SessionAction::SaveSession { name } => {
            tokio::spawn(async move {
                let config = {
                    let state = APP_STATE.state();
                    let app = state.read();
                    match app.active() {
                        Some(s) if s.session_type == SessionType::DirCompare => session_to_config(s),
                        _ => return,
                    }
                };

                let session_dir = app::default_session_dir();
                if std::fs::create_dir_all(&session_dir).is_err() {
                    return;
                }

                let path = session_dir.join(format!("{}.toml", name));
                match config.save_to_file(&path).await {
                    Ok(_) => {}
                    Err(e) => {
                        let state = APP_STATE.state();
                        let mut app = state.write();
                        if let Some(s) = app.active_mut() {
                            s.errors.push(format!("Failed to save session: {e}"));
                        }
                    }
                }
            });
        }
        SessionAction::ListSessions => {
            tokio::spawn(async move {
                match list_saved_sessions().await {
                    Ok(sessions) => {
                        let state = APP_STATE.state();
                        let mut app = state.write();
                        if let Some(session) = app.active_mut() {
                            session.filter_input = sessions
                                .iter()
                                .map(|p| {
                                    p.file_stem()
                                        .unwrap_or_default()
                                        .to_string_lossy()
                                        .to_string()
                                })
                                .collect::<Vec<_>>()
                                .join(",");
                        }
                    }
                    Err(_) => {}
                }
            });
        }
        SessionAction::LoadSession { path } => {
            tokio::spawn(async move {
                let config = match SessionConfig::load_from_file(&path).await {
                    Ok(c) => c,
                    Err(e) => {
                        let state = APP_STATE.state();
                        let mut app = state.write();
                        if let Some(s) = app.active_mut() {
                            s.errors.push(format!("Session load failed: {e}"));
                        }
                        return;
                    }
                };
                match create_session_from_config(&config).await {
                    Ok(session) => APP_STATE.state().write().add_session(session),
                    Err(e) => {
                        let state = APP_STATE.state();
                        let mut app = state.write();
                        if let Some(s) = app.active_mut() {
                            s.errors.push(format!("Session load failed: {e}"));
                        }
                    }
                }
            });
        }
        SessionAction::None => {}
    }
}

// ---------------------------------------------------------------------------
// DirCompareView component
// ---------------------------------------------------------------------------

/// Props for [`DirCompareView`] — none; state is read from the global atom.
#[derive(Default, Props)]
struct DirCompareViewProps;

/// Directory comparison view using ratatui-kit's `Table` component.
///
/// Subscribes to the global [`APP_STATE`] atom, uses a local
/// `State<TableState>` for table selection that is synced from the session on
/// each render, and registers an event handler that delegates to
/// `handle_session_key` for all session-level key processing.
#[component]
fn DirCompareView(mut hooks: Hooks) -> impl Into<AnyElement<'static>> {
    // Subscribe to global state for re-render notifications on writes.
    let app_handle = hooks.use_atom(&APP_STATE);

    // Local table selection state, synced from the session's table_state.
    let table_state = hooks.use_state(TableState::default);

    // Read session data from the atom.
    let app = app_handle.read();
    let active_tab = app.active_tab;
    let session = match app.active() {
        Some(s) if s.session_type == SessionType::DirCompare => s,
        _ => {
            drop(app);
            return element!(View {});
        }
    };

    // Sync local table state from the session's table_state (no re-render trigger).
    let session_selected = session.table_state.selected();
    if table_state.read().selected() != session_selected {
        table_state.write_no_update().select(session_selected);
    }

    // Extract rendering data from the session.
    let sync_active = session.sync_planned.is_some();
    let mode = session.mode.clone();
    let focus = session.focus;
    let hide_same = session.hide_same;
    let active_filter = session.active_filter.clone();
    let left_path_str = session.left_path.to_string_lossy().to_string();
    let right_path_str = session.right_path.to_string_lossy().to_string();
    let sync_operation = session.sync_operation;
    let entries: Vec<DirEntry> = session.filtered_entries().into_iter().cloned().collect();
    let sync_items: Vec<TransferItem> = session
        .sync_planned
        .as_ref()
        .map(|s| s.planned.clone())
        .unwrap_or_default();

    // Extract data needed after dropping the app handle (for status bar and
    // element! rendering). All session references below use these locals.
    let filter_input = session.filter_input.clone();
    let sync_planned_count = session
        .sync_planned
        .as_ref()
        .map(|s| s.planned_count())
        .unwrap_or(0);
    let sync_executed = session
        .sync_planned
        .as_ref()
        .map(|s| s.transfer.is_some())
        .unwrap_or(false);
    let comparison_stats = session.comparison.as_ref().map(|c| {
        format!(
            "Total: {}  Same: {}  Diff: {}  Orphans: {}",
            c.total(),
            c.same_count(),
            c.different_count(),
            c.orphan_count(),
        )
    });
    let comparison_is_none = session.comparison.is_none();

    drop(app);

    // Tab-switch sync: when the active tab changes, re-sync local table state
    // from the new session's table_state.
    let ts_for_effect = table_state;
    hooks.use_effect(
        move || {
            let state = APP_STATE.state();
            let app_read = state.read();
            if let Some(s) = app_read.active() {
                if s.session_type == SessionType::DirCompare {
                    let selected = s.table_state.selected();
                    ts_for_effect.write_no_update().select(selected);
                }
            }
        },
        active_tab,
    );

    // Event handler for session-level keys (j/k, g/G, Enter, f, F, etc.).
    // The App component's global handler runs first and consumes global keys
    // (Ctrl+W, Ctrl+Tab, Left/Right, q). For remaining keys, this handler
    // delegates to `handle_session_key`, which modifies session state in the
    // atom and returns a `SessionAction` for async processing.
    hooks.use_event_handler(
        EventScope::Current,
        EventPriority::Normal,
        move |event| {
            let Event::Key(key) = event else {
                return EventResult::Ignored;
            };
            if key.kind != KeyEventKind::Press {
                return EventResult::Ignored;
            }

            let state = APP_STATE.state();
            let mut app = state.write();
            let session = match app.active_mut() {
                Some(s) if s.session_type == SessionType::DirCompare => s,
                _ => return EventResult::Ignored,
            };

            let action = handle_session_key(session, key);
            drop(app);

            process_session_action(action);
            EventResult::Consumed
        },
    );

    // -- Build header lines --------------------------------------------------

    let header_lines = [
        Line::from(vec![
            Span::styled("cocomo", Style::new().add_modifier(Modifier::BOLD)),
            Span::raw(" \u{2014} "),
            Span::styled("Folder Compare", Style::new().add_modifier(Modifier::DIM)),
        ]),
        Line::from(vec![Span::styled(
            format!("Left:  {left_path_str}"),
            if focus == Focus::Left {
                Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD)
            } else {
                Style::new()
            },
        )]),
        Line::from(vec![Span::styled(
            format!("Right: {right_path_str}"),
            if focus == Focus::Right {
                Style::new().fg(Color::Magenta).add_modifier(Modifier::BOLD)
            } else {
                Style::new()
            },
        )]),
    ];

    // -- Build filter bar line ----------------------------------------------

    let _filter_bar_line: Line<'static> = if hide_same {
        Line::from(Span::styled("No Orphans", Style::new().fg(Color::Yellow)))
    } else {
        Line::from(Span::raw("Show Orphans"))
    };
    let _filter_bar_line = if let Some(filter) = &active_filter {
        Line::from(vec![
            Span::raw("Show Orphans".to_string()),
            Span::raw(" | "),
            Span::styled(format!("*{filter}*"), Style::new().fg(Color::Cyan)),
        ])
    } else {
        _filter_bar_line
    };

    // -- Build status bar line ----------------------------------------------

    let status_bar_line: Line<'static> = if mode == AppMode::Filter {
        Line::from(Span::styled(
            format!("Filter: {}", filter_input),
            Style::new().fg(Color::Yellow),
        ))
    } else if mode == AppMode::SaveSession {
        Line::from(Span::styled(
            format!("Save session as: {}", filter_input),
            Style::new().fg(Color::Yellow),
        ))
    } else if mode == AppMode::LoadSession {
        Line::from(Span::styled(
            format!("Load session: {}", filter_input),
            Style::new().fg(Color::Yellow),
        ))
    } else if sync_active {
        let count = sync_planned_count;
        let executed = sync_executed;
        let op_label = sync_operation.label();
        let status_text = if executed {
            "Executed".to_string()
        } else {
            format!("Planned: {} transfers", count)
        };
        Line::from(Span::styled(
            format!("Sync: {op_label}  |  {status_text}"),
            Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        ))
    } else {
        let stats = comparison_stats;
        let filter_info = active_filter
            .as_ref()
            .map(|f| format!("*{f}*"))
            .unwrap_or_else(|| "No filter".to_string());
        let display = if hide_same {
            "Hiding identical"
        } else {
            "Showing all"
        };
        Line::from(vec![
            Span::raw(stats.unwrap_or_else(|| "No data".to_string())),
            Span::raw("  |  "),
            Span::raw(filter_info),
            Span::raw("  |  "),
            Span::raw(display),
        ])
    };

    // -- Build help bar line -----------------------------------------------

    let help_bar_line: Line<'static> = if mode == AppMode::Filter {
        Line::from("Enter=apply  Esc=cancel")
    } else if mode == AppMode::SaveSession {
        Line::from("Enter=save  Esc=cancel")
    } else if mode == AppMode::LoadSession {
        Line::from("j/k=select  Enter=load  Esc=cancel")
    } else if sync_active {
        Line::from(vec![
            Span::styled("j/k", Style::new().add_modifier(Modifier::BOLD)),
            Span::raw(" navigate  "),
            Span::styled("Esc", Style::new().add_modifier(Modifier::BOLD)),
            Span::raw(" cancel"),
        ])
    } else {
        Line::from(vec![
            Span::styled("j/k", Style::new().add_modifier(Modifier::BOLD)),
            Span::raw(" navigate  "),
            Span::styled("l/Tab", Style::new().add_modifier(Modifier::BOLD)),
            Span::raw(" focus  "),
            Span::styled("f", Style::new().add_modifier(Modifier::BOLD)),
            Span::raw(" filter  "),
            Span::styled("F", Style::new().add_modifier(Modifier::BOLD)),
            Span::raw(" hide same  "),
            Span::styled("C", Style::new().add_modifier(Modifier::BOLD)),
            Span::raw(" cont. on/off  "),
            Span::styled("m/M", Style::new().add_modifier(Modifier::BOLD)),
            Span::raw(" sync op  "),
            Span::styled("P", Style::new().add_modifier(Modifier::BOLD)),
            Span::raw(" plan  "),
            Span::styled("e", Style::new().add_modifier(Modifier::BOLD)),
            Span::raw(" execute  "),
            Span::styled("r", Style::new().add_modifier(Modifier::BOLD)),
            Span::raw(" reload  "),
            Span::styled("s", Style::new().add_modifier(Modifier::BOLD)),
            Span::raw(" save  "),
            Span::styled("O", Style::new().add_modifier(Modifier::BOLD)),
            Span::raw(" load  "),
            Span::styled("\u{21A5}", Style::new().add_modifier(Modifier::BOLD)),
            Span::raw(" open dir  "),
            Span::styled("\u{232B}", Style::new().add_modifier(Modifier::BOLD)),
            Span::raw(" up  "),
            Span::styled("Ctrl+W", Style::new().add_modifier(Modifier::BOLD)),
            Span::raw(" close  "),
            Span::styled("Ctrl+Tab", Style::new().add_modifier(Modifier::BOLD)),
            Span::raw(" tabs  "),
            Span::styled("q", Style::new().add_modifier(Modifier::BOLD)),
            Span::raw(" quit"),
        ])
    };

    // -- Build the element tree ---------------------------------------------

    // Compute the empty-state message before element! (let bindings are not
    // allowed inside element! children blocks).
    let empty_msg = if comparison_is_none {
        "Loading comparison\u{2026}"
    } else if hide_same {
        "No differing entries."
    } else if active_filter.is_some() {
        "No entries match the current filter."
    } else {
        "Directories are identical."
    };

    let border_style = Style::new().fg(Color::Gray);
    let title_style = Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD);
    let comparison_block = Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(" Comparison ", title_style))
        .border_style(border_style);
    let sync_block = Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(" Sync Preview (dry-run) ", title_style))
        .border_style(border_style);

    element!(
        View(flex_direction: Direction::Vertical, height: Constraint::Fill(1)) {
            // Header (3 content lines + 2 border lines = 5).
            Border(
                height: Constraint::Length(5),
                flex_direction: Direction::Vertical,
                border_style: border_style,
            ) {
                Text(text: header_lines[0].clone())
                Text(text: header_lines[1].clone())
                Text(text: header_lines[2].clone())
            }
            // Main content area.
            if sync_active {
                Table<TransferItem>(
                    height: Constraint::Fill(1),
                    state: Some(table_state),
                    active: false,
                    default_index: Some(0),
                    columns: vec![
                        TableColumn::new("Action", Constraint::Length(30)),
                        TableColumn::new("Path", Constraint::Min(20)),
                    ],
                    rows: sync_items,
                    render_row: Some(Arc::new(|item: &TransferItem, _is_selected: bool| {
                        let action_str = item.action.label().to_string();
                        let dir_indicator = if item.is_dir { "\u{1F4C4} " } else { "  " };
                        let action_color = match item.action {
                            TransferAction::CopyLeft | TransferAction::CopyRight => Color::Green,
                            TransferAction::CopyCenter => Color::Yellow,
                            TransferAction::DeleteLeft | TransferAction::DeleteRight => Color::Red,
                            TransferAction::MoveLeft | TransferAction::MoveRight => Color::Magenta,
                        };
                        vec![
                            TableCell::new(action_str)
                                .style(Style::new().fg(action_color).add_modifier(Modifier::BOLD)),
                            TableCell::new(format!("{}{}", dir_indicator, &item.rel_path)),
                        ]
                    }) as RenderTableRow<TransferItem>),
                    block: Some(sync_block),
                    highlight_style: Style::new().bg(Color::DarkGray).add_modifier(Modifier::REVERSED),
                    highlight_symbol: Some(symbols::block::FULL),
                    column_spacing: 1u16,
                    header_style: Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD),
                )
            } else if entries.is_empty() {
                Border(
                    height: Constraint::Fill(1),
                    border_style: border_style,
                    top_title: Line::from(" Comparison ").fg(Color::Yellow).bold().centered(),
                ) {
                    Text(
                        text: Line::from(empty_msg),
                        alignment: Alignment::Center,
                    )
                }
            } else {
                Table<DirEntry>(
                    height: Constraint::Fill(1),
                    state: Some(table_state),
                    active: false,
                    default_index: Some(0),
                    columns: vec![
                        TableColumn::new("T Name", Constraint::Min(20)),
                        TableColumn::new(
                            format!("{:>6} {:^17}", "Size", "Modified"),
                            Constraint::Length(24),
                        ),
                        TableColumn::new("S", Constraint::Length(2)),
                        TableColumn::new(
                            format!("{:>6} {:^17}", "Size", "Modified"),
                            Constraint::Length(24),
                        ),
                    ],
                    rows: entries,
                    render_row: Some(Arc::new(|entry: &DirEntry, _is_selected: bool| {
                        let status_color = session_view::status_style(entry.status);

                        let left_meta = entry.left.as_ref().map(|l| {
                            format!(
                                "{:>6} {}",
                                session_view::format_size(l.size),
                                session_view::format_date(&l.modified)
                            )
                        }).unwrap_or_else(|| "-             ".to_string());

                        let right_meta = entry.right.as_ref().map(|r| {
                            format!(
                                "{:>6} {}",
                                session_view::format_size(r.size),
                                session_view::format_date(&r.modified)
                            )
                        }).unwrap_or_else(|| "-             ".to_string());

                        let dir_indicator = if entry.left.as_ref().is_some_and(|l| l.is_dir)
                            || entry.right.as_ref().is_some_and(|r| r.is_dir)
                        {
                            "\u{1F5C0}"
                        } else {
                            " "
                        };

                        vec![
                            TableCell::new(format!("{} {}", dir_indicator, &entry.name)),
                            TableCell::new(left_meta),
                            TableCell::new(entry.status.symbol())
                                .style(Style::new().fg(status_color).add_modifier(Modifier::BOLD)),
                            TableCell::new(right_meta),
                        ]
                    }) as RenderTableRow<DirEntry>),
                    block: Some(comparison_block),
                    highlight_style: Style::new().bg(Color::DarkGray).add_modifier(Modifier::REVERSED),
                    highlight_symbol: Some(symbols::block::FULL),
                    column_spacing: 1u16,
                    header_style: Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD),
                )
            }
            // Status bar.
            Border(height: Constraint::Length(3), border_style: border_style) {
                Text(text: status_bar_line)
            }
            // Help bar.
            Border(height: Constraint::Length(3), border_style: border_style) {
                Text(text: help_bar_line)
            }
        }
    )
}

// ---------------------------------------------------------------------------
// App component (root component)
// ---------------------------------------------------------------------------

/// Root component: subscribes to global state, registers global key handlers,
/// and renders the menu bar, tab bar, and main content.
#[component]
fn App(mut hooks: Hooks) -> impl Into<AnyElement<'static>> {
    let app_state = hooks.use_atom(&APP_STATE);
    let mut exit = hooks.use_exit();

    // Register global key handler: Ctrl+W (close tab), Ctrl+Tab (switch tabs),
    // Left/Right (menu nav), q (quit).
    hooks.use_event_handler(
        EventScope::Current,
        EventPriority::Normal,
        move |event| {
            let Event::Key(key) = event else {
                return EventResult::Ignored;
            };
            if key.kind != KeyEventKind::Press {
                return EventResult::Ignored;
            }

            let state = APP_STATE.state();
            let mut app = state.write();
            match handle_global_key(&mut app, key) {
                GlobalKeyResult::Quit => {
                    drop(app);
                    exit();
                    EventResult::Consumed
                }
                GlobalKeyResult::Consumed => EventResult::Consumed,
                GlobalKeyResult::Ignored => EventResult::Ignored,
            }
        },
    );

    // Read current state for rendering the menu and tab bars.
    let app = app_state.read();
    let menu_selection = app.menu_selection;
    let active_index = app.active_index();
    let active_session_type = app.active().map(|s| s.session_type);

    // Build menu bar tab titles.
    let menu_titles: Vec<Line<'static>> =
        MENU_TITLES.iter().map(|s| Line::from(*s)).collect();

    // Build session tab titles.
    let tab_titles: Vec<Line<'static>> = app
        .sessions
        .iter()
        .map(|s| Line::from(s.tab_title().to_string()))
        .collect();

    drop(app);

    element!(
        View(flex_direction: Direction::Vertical) {
            MenuBar(menu_titles: menu_titles, selected: menu_selection)
            TabBar(tab_titles: tab_titles, active_index: active_index)
            View(height: Constraint::Min(1)) {
                match active_session_type {
                    Some(SessionType::DirCompare) => { DirCompareView() }
                    _ => { SessionContent() }
                }
            }
        }
    )
}

// ---------------------------------------------------------------------------
// Main entry point
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    let has_paths = cli.left.is_some() && cli.right.is_some();
    let (left, right) = if has_paths {
        let l = cli.left.unwrap();
        let r = cli.right.unwrap();
        let left = tokio::fs::canonicalize(&l).await?;
        let right = tokio::fs::canonicalize(&r).await?;

        // Validate that both paths exist and are of the same type.
        let left_meta = tokio::fs::metadata(&left).await?;
        let right_meta = tokio::fs::metadata(&right).await?;

        if left_meta.is_file() && right_meta.is_file() {
            return Err(anyhow::anyhow!(
                "Comparing individual files is not yet implemented. Please \
                 provide directories."
            ));
        }

        if !left_meta.is_dir() || !right_meta.is_dir() {
            return Err(anyhow::anyhow!(
                "Both --left and --right must be directories, or both must \
                 be files."
            ));
        }

        (left, right)
    } else {
        (PathBuf::from("."), PathBuf::from("."))
    };

    let mut app = app::App::new();

    // Create the initial comparison session if paths were provided.
    if has_paths {
        let mut session =
            SessionView::new_dir_compare(left, right, cli.content);
        if let Err(e) = run_comparison(&mut session).await {
            session.errors.push(format!("Comparison failed: {e}"));
        }
        app.add_session(session);
    }

    APP_STATE.set(app);

    element!(App).fullscreen().await?;

    Ok(())
}
