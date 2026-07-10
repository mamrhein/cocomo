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

use std::{
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
};

use anyhow::Result;
use app::{run_comparison, run_text_comparison, App, SessionAction, SessionView};
use clap::Parser;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    text::Line,
    widgets::{Block, Borders, Paragraph},
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

    /// Structure-only comparison (skip content hashing).
    #[arg(short = 's', long, default_value_t = false)]
    structure_only: bool,
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

fn render(frame: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1)].as_ref())
        .split(frame.area());

    render_tab_bar(frame, chunks[0], app);

    if let Some(session) = app.active() {
        match session.session_type {
            app::SessionType::DirCompare => {
                session_view::render_session(frame, chunks[1], session);
            }
            app::SessionType::TextCompare => {
                session_view::render_text_session(frame, chunks[1], session);
            }
        }
    } else {
        // No sessions open — show welcome screen.
        render_welcome(frame, chunks[1]);
    }
}

fn render_tab_bar(frame: &mut Frame, area: Rect, app: &App) {
    let sessions = &app.sessions;
    let active = app.active_index();

    if sessions.is_empty() {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Gray));
        frame.render_widget(block, area);
        return;
    }

    // Build tab labels.
    let tabs: Vec<_> = sessions
        .iter()
        .enumerate()
        .map(|(i, s)| {
            if i == active {
                format!(" {} ", s.tab_title())
            } else {
                format!("  {}  ", s.tab_title())
            }
        })
        .collect();

    let line = Line::from(tabs.join("│"));
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Gray));
    let paragraph = Paragraph::new(line)
        .block(block)
        .style(if active < sessions.len() {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default()
        });
    frame.render_widget(paragraph, area);
}

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
        .alignment(ratatui::layout::Alignment::Center);
    frame.render_widget(paragraph, area);
}

// ---------------------------------------------------------------------------
// Global key handling
// ---------------------------------------------------------------------------

/// Handle keys that affect the app globally (tab switching, quit).
///
/// Returns `true` if the key was consumed by a global handler, `false` if it
/// should be dispatched to the active session.
fn handle_global_key(app: &mut App, key: KeyEvent) -> bool {
    // Ctrl+W — close active tab.
    if key.code == KeyCode::Char('w')
        && key.modifiers.contains(KeyModifiers::CONTROL)
    {
        app.close_active();
        return true;
    }

    // Ctrl+Tab — next tab.
    if key.code == KeyCode::Tab && key.modifiers.contains(KeyModifiers::CONTROL) {
        let is_shift = key.modifiers.contains(KeyModifiers::SHIFT);
        app.switch_tab(if is_shift { -1 } else { 1 });
        return true;
    }

    // q — quit (only when not in a session filter mode).
    if key.code == KeyCode::Char('q') || key.code == KeyCode::Char('Q') {
        // If there's an active session in filter mode, let the session handle it.
        if let Some(session) = app.active() {
            if session.mode != app::AppMode::Filter {
                app.running = false;
                return true;
            }
        } else {
            app.running = false;
            return true;
        }
    }

    false
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

/// Global flag indicating whether the terminal has been set up.
static TERMINAL_ACTIVE: AtomicBool = AtomicBool::new(false);

fn install_panic_hook() {
    let original = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = teardown_terminal();
        TERMINAL_ACTIVE.store(false, Ordering::Relaxed);
        original(info);
    }));
}

struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = teardown_terminal();
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    install_panic_hook();

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
                "Comparing individual files is not yet implemented. Please provide directories."
            ));
        }

        if !left_meta.is_dir() || !right_meta.is_dir() {
            return Err(anyhow::anyhow!(
                "Both --left and --right must be directories, or both must be files."
            ));
        }

        (left, right)
    } else {
        (PathBuf::from("."), PathBuf::from("."))
    };

    let _terminal_guard = TerminalGuard;
    let mut terminal = setup_terminal()?;

    let mut app_state = App::new();

    // Create the initial comparison session if paths were provided.
    if has_paths {
        let mut session =
            SessionView::new_dir_compare(left, right, !cli.structure_only);
        if let Err(e) = run_comparison(&mut session).await {
            session.errors.push(format!("Comparison failed: {e}"));
        }
        app_state.add_session(session);
    }

    // Main event loop.
    loop {
        terminal.draw(|frame| render(frame, &app_state))?;

        if event::poll(std::time::Duration::from_millis(100))?
            && matches!(event::read().ok(), Some(Event::Key(key)) if key.kind == KeyEventKind::Press)
        {
            let key = match event::read()? {
                Event::Key(k) if k.kind == KeyEventKind::Press => k,
                _ => continue,
            };

            // Global keys first.
            if handle_global_key(&mut app_state, key) {
                if !app_state.running {
                    break;
                }
                continue;
            }

            // Dispatch to active session.
            if let Some(session) = app_state.active_mut() {
                let action = session_view::handle_session_key(session, key);

                // Handle async actions.
                match action {
                    SessionAction::OpenDir { left, right } => {
                        let mut new_session =
                            SessionView::new_dir_compare(
                                left,
                                right,
                                session.compare_files,
                            );
                        if let Err(e) =
                            run_comparison(&mut new_session).await
                        {
                            new_session
                                .errors
                                .push(format!("Comparison failed: {e}"));
                        }
                        app_state.add_session(new_session);
                    }
                    SessionAction::GoUp => {
                        let left_parent = session.left_path.parent();
                        let right_parent = session.right_path.parent();
                        if let (Some(l), Some(r)) = (left_parent, right_parent) {
                            // Avoid going above root.
                            if l != session.left_path.as_path()
                                || r != session.right_path.as_path()
                            {
                                let mut new_session =
                                    SessionView::new_dir_compare(
                                        l.to_path_buf(),
                                        r.to_path_buf(),
                                        session.compare_files,
                                    );
                                if let Err(e) =
                                    run_comparison(&mut new_session).await
                                {
                                    new_session.errors
                                        .push(format!("Comparison failed: {e}"));
                                }
                                app_state.add_session(new_session);
                            }
                        }
                    }
                    SessionAction::OpenFile { left, right } => {
                        match run_text_comparison(left, right).await {
                            Ok(new_session) => {
                                app_state.add_session(new_session);
                            }
                            Err(e) => {
                                session
                                    .errors
                                    .push(format!("Text comparison failed: {e}"));
                            }
                        }
                    }
                    SessionAction::Reload => {
                        if let Err(e) = run_comparison(session).await {
                            session.errors.push(format!("Reload failed: {e}"));
                        }
                    }
                    SessionAction::None => {}
                }
            }

            if !app_state.running {
                break;
            }
        }
    }

    Ok(())
}

fn setup_terminal() -> Result<Terminal<CrosstermBackend<std::io::Stdout>>> {
    crossterm::terminal::enable_raw_mode()?;
    crossterm::execute!(
        std::io::stdout(),
        crossterm::terminal::EnterAlternateScreen,
    )?;
    let backend = CrosstermBackend::new(std::io::stdout());
    let terminal = Terminal::new(backend)?;
    TERMINAL_ACTIVE.store(true, Ordering::Relaxed);
    Ok(terminal)
}

fn teardown_terminal() -> Result<()> {
    if !TERMINAL_ACTIVE.swap(false, Ordering::Relaxed) {
        return Ok(());
    }
    crossterm::terminal::disable_raw_mode()?;
    crossterm::execute!(
        std::io::stdout(),
        crossterm::terminal::LeaveAlternateScreen,
    )?;
    Ok(())
}
