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
//! Provides a side-by-side folder comparison view with navigation, status
//! indicators, and basic filtering.

use std::{path::PathBuf, sync::Arc};

use anyhow::Result;
use clap::Parser;
use cocomo_lib::{
    CompareConfig, ContentCache, DirComparison, DirEntry, DirEntryStatus,
    FileSystem, LocalFs,
};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Row, Table, TableState},
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
// Application State
// ---------------------------------------------------------------------------

/// Which pane currently has focus.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum Focus {
    #[default]
    Left,
    Right,
}

/// The current mode of the application.
#[derive(Clone, Debug, Default, PartialEq)]
enum AppMode {
    #[default]
    Normal,
    /// Filter input mode.
    Filter,
}

struct App {
    /// `false` when the app should exit.
    running: bool,
    /// Current keyboard focus.
    focus: Focus,
    /// Current application mode.
    mode: AppMode,
    /// Filter text input buffer.
    filter_input: String,
    /// Active filter pattern. `None` means no filter.
    active_filter: Option<String>,
    /// Hide identical entries.
    hide_same: bool,
    /// Comparison result.
    comparison: Option<DirComparison>,
    /// Comparison errors.
    errors: Vec<String>,
    /// Selected row in the entry table.
    table_state: TableState,
    /// Current comparison paths.
    current_path_left: Option<PathBuf>,
    current_path_right: Option<PathBuf>,
    /// Compare file contents (vs. structure only).
    compare_files: bool,
    /// Whether a reload was requested.
    reload_requested: bool,
}

impl App {
    fn new() -> Self {
        Self {
            running: true,
            focus: Focus::default(),
            mode: AppMode::default(),
            filter_input: String::new(),
            active_filter: None,
            hide_same: false,
            comparison: None,
            errors: Vec::new(),
            table_state: TableState::default(),
            current_path_left: None,
            current_path_right: None,
            compare_files: true,
            reload_requested: false,
        }
    }

    /// Get the filtered entries for display.
    fn filtered_entries(&self) -> Vec<&DirEntry> {
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
}

// ---------------------------------------------------------------------------
// Key handling
// ---------------------------------------------------------------------------

fn handle_filter_input(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => {
            app.filter_input.clear();
            app.mode = AppMode::Normal;
        }
        KeyCode::Enter => {
            if app.filter_input.is_empty() {
                app.active_filter = None;
            } else {
                app.active_filter = Some(app.filter_input.clone());
            }
            app.filter_input.clear();
            app.mode = AppMode::Normal;
            app.table_state.select(Some(0));
        }
        KeyCode::Backspace => {
            app.filter_input.pop();
        }
        KeyCode::Char(c) => {
            app.filter_input.push(c);
        }
        _ => {}
    }
}

fn handle_key_input(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Char('q') | KeyCode::Char('Q') => {
            app.running = false;
        }
        KeyCode::Char('j') | KeyCode::Down => {
            let entries = app.filtered_entries();
            if let Some(selected) = app.table_state.selected() {
                if selected + 1 < entries.len() {
                    app.table_state.select(Some(selected + 1));
                }
            } else if !entries.is_empty() {
                app.table_state.select(Some(0));
            }
        }
        KeyCode::Char('k') | KeyCode::Up => {
            if let Some(selected) =
                app.table_state.selected().filter(|s| *s > 0)
            {
                app.table_state.select(Some(selected - 1));
            }
        }
        KeyCode::Char('g') => {
            app.table_state.select(Some(0));
        }
        KeyCode::Char('G') => {
            let entries = app.filtered_entries();
            if !entries.is_empty() {
                app.table_state.select(Some(entries.len() - 1));
            }
        }
        KeyCode::Char('l') => {
            app.focus = match app.focus {
                Focus::Left => Focus::Right,
                Focus::Right => Focus::Left,
            };
        }
        KeyCode::Char('f') => {
            app.mode = AppMode::Filter;
            app.filter_input.clear();
        }
        KeyCode::Char('F') => {
            app.hide_same = !app.hide_same;
            app.table_state.select(Some(0));
        }
        KeyCode::Char('S') => {
            app.compare_files = !app.compare_files;
        }
        KeyCode::Char('r') => {
            app.reload_requested = true;
        }
        KeyCode::Esc => {
            app.active_filter = None;
            app.table_state.select(Some(0));
        }
        KeyCode::Tab => {
            app.focus = match app.focus {
                Focus::Left => Focus::Right,
                Focus::Right => Focus::Left,
            };
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// UI rendering
// ---------------------------------------------------------------------------

fn render(frame: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(1),
            Constraint::Length(3),
        ])
        .split(frame.area());

    render_header(frame, chunks[0], app);
    render_main(frame, chunks[1], app);
    render_footer(frame, chunks[2], app);
}

fn render_header(frame: &mut Frame, area: Rect, app: &App) {
    let left_path = app
        .current_path_left
        .as_deref()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| "not set".to_string());
    let right_path = app
        .current_path_right
        .as_deref()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| "not set".to_string());

    let lines = vec![
        Line::from(vec![
            Span::styled(
                "cocomo",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(" — "),
            Span::styled(
                "Folder Compare",
                Style::default().add_modifier(Modifier::DIM),
            ),
        ]),
        Line::from(vec![Span::styled(
            format!("Left:  {left_path}"),
            if app.focus == Focus::Left {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            },
        )]),
        Line::from(vec![Span::styled(
            format!("Right: {right_path}"),
            if app.focus == Focus::Right {
                Style::default()
                    .fg(Color::Magenta)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            },
        )]),
    ];

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Gray));
    let paragraph = Paragraph::new(lines).block(block);
    frame.render_widget(paragraph, area);
}

fn render_main(frame: &mut Frame, area: Rect, app: &App) {
    let entries = app.filtered_entries();

    if entries.is_empty() {
        let msg = if app.comparison.is_none() {
            "No comparison loaded. Provide --left and --right paths."
        } else if app.hide_same {
            "No differing entries."
        } else if app.active_filter.is_some() {
            "No entries match the current filter."
        } else {
            "Directories are identical."
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .title(Span::styled(
                " Comparison ",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ))
            .border_style(Style::default().fg(Color::Gray));
        let paragraph = Paragraph::new(Line::from(msg))
            .block(block)
            .alignment(Alignment::Center);
        frame.render_widget(paragraph, area);
        return;
    }

    let rows = entries.iter().map(|entry| {
        let status_style = status_style(entry.status);
        let name = &entry.name;
        let left_size = entry
            .left
            .as_ref()
            .map(|l| format_size(l.size))
            .unwrap_or_else(|| "-".to_string());
        let right_size = entry
            .right
            .as_ref()
            .map(|r| format_size(r.size))
            .unwrap_or_else(|| "-".to_string());
        let left_date = entry
            .left
            .as_ref()
            .map(|l| format_date(&l.modified))
            .unwrap_or_else(|| "-".to_string());
        let right_date = entry
            .right
            .as_ref()
            .map(|r| format_date(&r.modified))
            .unwrap_or_else(|| "-".to_string());

        let dir_indicator = if entry.left.as_ref().is_some_and(|l| l.is_dir)
            || entry.right.as_ref().is_some_and(|r| r.is_dir)
        {
            "[+] "
        } else {
            "    "
        };

        Row::new(vec![
            Span::styled(
                entry.status.symbol(),
                Style::default()
                    .fg(status_style)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!("{dir_indicator}{name}")),
            Span::raw(left_size),
            Span::raw(left_date),
            Span::raw(right_size),
            Span::raw(right_date),
        ])
    });

    let header = Row::new(vec![
        Span::styled("S", Style::default().add_modifier(Modifier::BOLD)),
        Span::styled("Name", Style::default().add_modifier(Modifier::BOLD)),
        Span::styled(
            "Left Size",
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "Left Modified",
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "Right Size",
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "Right Modified",
            Style::default().add_modifier(Modifier::BOLD),
        ),
    ]);

    let widths = [
        Constraint::Length(2),
        Constraint::Min(20),
        Constraint::Length(10),
        Constraint::Length(20),
        Constraint::Length(10),
        Constraint::Length(20),
    ];

    let block = Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(
            " Comparison ",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ))
        .border_style(Style::default().fg(Color::Gray));

    let table = Table::new(rows, widths)
        .header(header)
        .block(block)
        .column_spacing(1)
        .row_highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::REVERSED),
        )
        .highlight_symbol(ratatui::symbols::block::FULL);

    frame.render_stateful_widget(table, area, &mut app.table_state.clone());
}

fn render_footer(frame: &mut Frame, area: Rect, app: &App) {
    let lines = if app.mode == AppMode::Filter {
        vec![
            Line::from(Span::styled(
                format!("Filter: {}", app.filter_input),
                Style::default().fg(Color::Yellow),
            )),
            Line::from(Span::raw("Enter=apply  Esc=cancel")),
        ]
    } else {
        let stats = app.comparison.as_ref().map(|c| {
            format!(
                "Total: {}  Same: {}  Diff: {}  Orphans: {}",
                c.total(),
                c.same_count(),
                c.different_count(),
                c.orphan_count(),
            )
        });

        let filter_info = app
            .active_filter
            .as_ref()
            .map(|f| format!("Filter: *{f}*"))
            .unwrap_or_else(|| "No filter".to_string());

        vec![
            Line::from(Span::raw(
                stats.unwrap_or_else(|| "No data".to_string()),
            )),
            Line::from(vec![
                Span::raw(filter_info),
                Span::raw("  |  "),
                Span::raw(if app.hide_same {
                    "Hiding identical"
                } else {
                    "Showing all"
                }),
            ]),
        ]
    };

    let help = Line::from(vec![
        Span::styled("j/k", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw(" navigate  "),
        Span::styled("l/Tab", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw(" focus  "),
        Span::styled("f", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw(" filter  "),
        Span::styled("F", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw(" hide same  "),
        Span::styled("S", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw(" toggle files/structure  "),
        Span::styled("r", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw(" reload  "),
        Span::styled("q", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw(" quit"),
    ]);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Gray));
    let paragraph = Paragraph::new([lines, vec![help]].concat()).block(block);
    frame.render_widget(paragraph, area);
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn status_style(status: DirEntryStatus) -> Color {
    match status {
        DirEntryStatus::Same | DirEntryStatus::SameBinary => Color::Gray,
        DirEntryStatus::Similar => Color::Yellow,
        DirEntryStatus::Different => Color::Red,
        DirEntryStatus::LeftOnly => Color::Cyan,
        DirEntryStatus::RightOnly => Color::Magenta,
        DirEntryStatus::CenterOnly => Color::White,
        DirEntryStatus::Mergeable => Color::Green,
        DirEntryStatus::Conflict => Color::Rgb(255, 0, 255),
        DirEntryStatus::IdenticalNameDifferentType => Color::Rgb(255, 255, 0),
    }
}

fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes}B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1}K", bytes as f64 / 1024.0)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.1}M", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.1}G", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}

fn format_date(date_str: &str) -> String {
    if date_str.len() > 16 {
        date_str[..16].replace('T', " ")
    } else {
        date_str.to_string()
    }
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    let left = cli.left.as_ref().ok_or_else(|| {
        anyhow::anyhow!("--left is required. Provide a directory path.")
    })?;
    let right = cli.right.as_ref().ok_or_else(|| {
        anyhow::anyhow!("--right is required. Provide a directory path.")
    })?;

    let mut terminal = setup_terminal()?;

    let mut app = App::new();
    app.current_path_left = Some(tokio::fs::canonicalize(left).await?);
    app.current_path_right = Some(tokio::fs::canonicalize(right).await?);
    app.compare_files = !cli.structure_only;

    if let Err(e) = run_comparison(&mut app, left.clone(), right.clone()).await
    {
        app.errors.push(format!("Comparison failed: {e}"));
    }

    loop {
        terminal.draw(|frame| render(frame, &app))?;

        // Poll for events.
        if event::poll(std::time::Duration::from_millis(100))?
            && matches!(event::read().ok(), Some(Event::Key(key)) if key.kind == KeyEventKind::Press)
        {
            let key = match event::read()? {
                Event::Key(k) if k.kind == KeyEventKind::Press => k,
                _ => continue,
            };

            match app.mode {
                AppMode::Filter => handle_filter_input(&mut app, key),
                AppMode::Normal => handle_key_input(&mut app, key),
            }

            if app.reload_requested
                && let (Some(l), Some(r)) = (
                    app.current_path_left.clone(),
                    app.current_path_right.clone(),
                )
            {
                app.reload_requested = false;
                if run_comparison(&mut app, l, r).await.is_err() {
                    app.errors.push("Reload failed.".to_string());
                }
            }

            if !app.running {
                break;
            }
        }
    }

    teardown_terminal()?;
    Ok(())
}

async fn run_comparison(
    app: &mut App,
    left: PathBuf,
    right: PathBuf,
) -> Result<()> {
    let fs: Arc<dyn FileSystem> = Arc::new(LocalFs::new("local"));
    let cache = ContentCache::default_config();

    let config = if app.compare_files {
        CompareConfig::full()
    } else {
        CompareConfig::structure_only()
    };

    let comparison =
        cocomo_lib::compare_directories(&fs, &left, &right, &config, &cache)
            .await?;

    app.comparison = Some(comparison);
    app.table_state.select(Some(0));

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
    Ok(terminal)
}

fn teardown_terminal() -> Result<()> {
    crossterm::terminal::disable_raw_mode()?;
    crossterm::execute!(
        std::io::stdout(),
        crossterm::terminal::LeaveAlternateScreen,
    )?;
    Ok(())
}
