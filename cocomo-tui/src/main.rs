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

use std::path::PathBuf;

use anyhow::Result;
use app::{APP_STATE, SessionView, run_comparison};
use clap::Parser;
use ratatui_kit::{
    crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    prelude::*,
    ratatui::{
        Frame,
        layout::{Alignment, Constraint, Direction, Rect},
        style::{Color, Modifier, Style},
        text::{Line, Span},
        widgets::{Block, Borders, Paragraph, Tabs},
    },
};

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
            if session.mode != app::AppMode::Filter {
                return GlobalKeyResult::Quit;
            }
        } else {
            return GlobalKeyResult::Quit;
        }
    }

    GlobalKeyResult::Ignored
}

// ---------------------------------------------------------------------------
// Content rendering (used by SessionContent component)
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
            app::SessionType::DirCompare => {
                session_view::render_session(frame, area, session);
            }
            app::SessionType::TextCompare => {
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
    let session_count = app.sessions.len();

    // Build menu bar tab titles.
    let menu_titles: Vec<Line<'static>> =
        ["Sessions", "Compare", "Sync", "View", "Filter", "Help"]
            .iter()
            .map(|s| Line::from(*s))
            .collect();

    // Build session tab titles.
    let tab_titles: Vec<Line<'static>> = app
        .sessions
        .iter()
        .map(|s| Line::from(s.tab_title().to_string()))
        .collect();

    drop(app);

    let menu_tabs = Tabs::new(menu_titles)
        .select(menu_selection)
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

    let tab_tabs = if session_count > 0 {
        Tabs::new(tab_titles)
            .select(active_index)
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
        View(flex_direction: Direction::Vertical) {
            View(height: Constraint::Length(3)) {
                widget(menu_tabs)
            }
            View(height: Constraint::Length(3)) {
                widget(tab_tabs)
            }
            View(height: Constraint::Min(1)) {
                SessionContent()
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
