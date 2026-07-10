// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Rendering and key handling for individual session views.
//!
//! Each session tab is rendered independently by this module. Key handling
//! returns [`SessionAction`] values that the main loop acts upon (e.g.,
//! creating new sessions for directory navigation).

use cocomo_lib::{DirEntryStatus, Importance, LineInfo, TextDifference};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Row, Table},
};

use crate::app::{AppMode, Focus, SessionAction, SessionView};

/// Render the active session's content (header, main table, footer).
pub fn render_session(frame: &mut Frame, area: Rect, session: &SessionView) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(1),
            Constraint::Length(3),
        ])
        .split(area);

    render_header(frame, chunks[0], session);
    render_main(frame, chunks[1], session);
    render_footer(frame, chunks[2], session);
}

fn render_header(frame: &mut Frame, area: Rect, session: &SessionView) {
    let left_path = session.left_path.to_string_lossy().to_string();
    let right_path = session.right_path.to_string_lossy().to_string();

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
            if session.focus == Focus::Left {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            },
        )]),
        Line::from(vec![Span::styled(
            format!("Right: {right_path}"),
            if session.focus == Focus::Right {
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

fn render_main(frame: &mut Frame, area: Rect, session: &SessionView) {
    let entries = session.filtered_entries();

    if entries.is_empty() {
        let msg = if session.comparison.is_none() {
            "Loading comparison…"
        } else if session.hide_same {
            "No differing entries."
        } else if session.active_filter.is_some() {
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

    frame.render_stateful_widget(
        table,
        area,
        &mut session.table_state.clone(),
    );
}

fn render_footer(frame: &mut Frame, area: Rect, session: &SessionView) {
    let lines = if session.mode == AppMode::Filter {
        vec![
            Line::from(Span::styled(
                format!("Filter: {}", session.filter_input),
                Style::default().fg(Color::Yellow),
            )),
            Line::from(Span::raw("Enter=apply  Esc=cancel")),
        ]
    } else {
        let stats = session.comparison.as_ref().map(|c| {
            format!(
                "Total: {}  Same: {}  Diff: {}  Orphans: {}",
                c.total(),
                c.same_count(),
                c.different_count(),
                c.orphan_count(),
            )
        });

        let filter_info = session
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
                Span::raw(if session.hide_same {
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
        Span::raw(" files/struct  "),
        Span::styled("r", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw(" reload  "),
        Span::styled("↵", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw(" open dir  "),
        Span::styled("⌫", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw(" up  "),
        Span::styled("Ctrl+W", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw(" close  "),
        Span::styled(
            "Ctrl+Tab",
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::raw(" tabs  "),
        Span::styled("q", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw(" quit"),
    ]);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Gray));
    let paragraph = Paragraph::new([lines, vec![help]].concat()).block(block);
    frame.render_widget(paragraph, area);
}

/// Handle key input for a session.
///
/// Returns a [`SessionAction`] if the key press requires the main loop to
/// perform an async operation (e.g., opening a new directory comparison).
pub fn handle_session_key(
    session: &mut SessionView,
    key: KeyEvent,
) -> SessionAction {
    if session.mode == AppMode::Filter {
        handle_filter_input(session, key);
        return SessionAction::None;
    }

    match key.code {
        KeyCode::Char('q') | KeyCode::Char('Q') => {
            // The main loop handles quit via a separate global key handler.
            SessionAction::None
        }
        KeyCode::Char('j') | KeyCode::Down => {
            navigate(session, 1);
            SessionAction::None
        }
        KeyCode::Char('k') | KeyCode::Up => {
            navigate(session, -1);
            SessionAction::None
        }
        KeyCode::Char('g') => {
            session.table_state.select(Some(0));
            SessionAction::None
        }
        KeyCode::Char('G') => {
            let entries = session.filtered_entries();
            if !entries.is_empty() {
                session.table_state.select(Some(entries.len() - 1));
            }
            SessionAction::None
        }
        KeyCode::Char('l') | KeyCode::Tab => {
            session.focus = match session.focus {
                Focus::Left => Focus::Right,
                Focus::Right => Focus::Left,
            };
            SessionAction::None
        }
        KeyCode::Char('f') => {
            session.mode = AppMode::Filter;
            session.filter_input.clear();
            SessionAction::None
        }
        KeyCode::Char('F') => {
            session.hide_same = !session.hide_same;
            session.table_state.select(Some(0));
            SessionAction::None
        }
        KeyCode::Char('S') => {
            session.compare_files = !session.compare_files;
            SessionAction::None
        }
        KeyCode::Char('r') => SessionAction::Reload,
        KeyCode::Enter => handle_enter(session),
        KeyCode::Backspace => handle_backspace(session),
        KeyCode::Esc => {
            session.active_filter = None;
            session.table_state.select(Some(0));
            SessionAction::None
        }
        _ => SessionAction::None,
    }
}

fn handle_filter_input(session: &mut SessionView, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => {
            session.filter_input.clear();
            session.mode = AppMode::Normal;
        }
        KeyCode::Enter => {
            if session.filter_input.is_empty() {
                session.active_filter = None;
            } else {
                session.active_filter = Some(session.filter_input.clone());
            }
            session.filter_input.clear();
            session.mode = AppMode::Normal;
            session.table_state.select(Some(0));
        }
        KeyCode::Backspace => {
            session.filter_input.pop();
        }
        KeyCode::Char(c) => {
            session.filter_input.push(c);
        }
        _ => {}
    }
}

fn navigate(session: &mut SessionView, delta: isize) {
    let entries = session.filtered_entries();
    if delta > 0 {
        if let Some(selected) = session.table_state.selected() {
            if selected + 1 < entries.len() {
                session.table_state.select(Some(selected + 1));
            }
        } else if !entries.is_empty() {
            session.table_state.select(Some(0));
        }
    } else if let Some(selected) =
        session.table_state.selected().filter(|s| *s > 0)
    {
        session.table_state.select(Some(selected - 1));
    }
}

/// Handle Enter key — open the selected directory in a new session.
fn handle_enter(session: &SessionView) -> SessionAction {
    let entries = session.filtered_entries();
    let selected = match session.table_state.selected() {
        Some(i) if i < entries.len() => entries[i],
        _ => return SessionAction::None,
    };

    // Build the sub-paths for the new comparison.
    let left_sub = selected
        .left
        .as_ref()
        .map(|l| std::path::PathBuf::from(&l.path))
        .or_else(|| {
            // Entry only exists on the right; use a non-existent left path.
            Some(session.left_path.join(&selected.name))
        });
    let right_sub = selected
        .right
        .as_ref()
        .map(|r| std::path::PathBuf::from(&r.path))
        .or_else(|| {
            // Entry only exists on the left; use a non-existent right path.
            Some(session.right_path.join(&selected.name))
        });

    let (left, right) = match (left_sub, right_sub) {
        (Some(l), Some(r)) => (l, r),
        _ => return SessionAction::None,
    };

    // Check if the selected entry is a directory.
    let is_dir = selected.left.as_ref().is_some_and(|l| l.is_dir)
        || selected.right.as_ref().is_some_and(|r| r.is_dir);

    if is_dir {
        SessionAction::OpenDir { left, right }
    } else {
        // It's a file — open a text diff session.
        SessionAction::OpenFile { left, right }
    }
}

/// Handle Backspace — navigate to parent directories in a new session.
fn handle_backspace(_session: &SessionView) -> SessionAction {
    // The main loop has access to the session paths and handles this
    // by creating a new session with parent directories.
    SessionAction::GoUp
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

pub fn status_style(status: DirEntryStatus) -> Color {
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

pub fn format_size(bytes: u64) -> String {
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

pub fn format_date(date_str: &str) -> String {
    if date_str.len() > 16 {
        date_str[..16].replace('T', " ")
    } else {
        date_str.to_string()
    }
}

// ---------------------------------------------------------------------------
// Text diff rendering
// ---------------------------------------------------------------------------

/// Render a text comparison session.
pub fn render_text_session(
    frame: &mut Frame,
    area: Rect,
    session: &SessionView,
) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(1),
            Constraint::Length(2),
        ])
        .split(area);

    render_text_header(frame, chunks[0], session);
    render_text_main(frame, chunks[1], session);
    render_text_footer(frame, chunks[2], session);
}

fn render_text_header(frame: &mut Frame, area: Rect, session: &SessionView) {
    let left_name = session
        .left_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "(empty)".to_string());
    let right_name = session
        .right_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "(empty)".to_string());

    let left_lines = session.left_lines.len();
    let right_lines = session.right_lines.len();

    let lines = vec![Line::from(format!(
        "Text Diff  |  {}: {left_lines}L  |  {}: {right_lines}L  |  Grammar: \
         {}",
        left_name, right_name, session.grammar.name,
    ))];

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Gray));
    let paragraph = Paragraph::new(lines).block(block);
    frame.render_widget(paragraph, area);
}

/// Build a set of line numbers that are part of a difference.
fn diff_line_numbers(
    session: &SessionView,
) -> std::collections::HashSet<usize> {
    let mut numbers = std::collections::HashSet::new();

    if let Some(ref diff) = session.text_diff {
        for td in &diff.differences {
            match td {
                TextDifference::LineDifferent(left, right) => {
                    numbers.insert(left.number);
                    numbers.insert(right.number);
                }
                TextDifference::LinesAdded(_, lines) => {
                    for l in lines {
                        numbers.insert(l.number);
                    }
                }
                TextDifference::LinesRemoved(_, lines) => {
                    for l in lines {
                        numbers.insert(l.number);
                    }
                }
                TextDifference::LinesChanged(_, removed, added) => {
                    for l in removed {
                        numbers.insert(l.number);
                    }
                    for l in added {
                        numbers.insert(l.number);
                    }
                }
                TextDifference::BlankLine => {}
            }
        }
    }

    numbers
}

fn render_text_main(frame: &mut Frame, area: Rect, session: &SessionView) {
    let diff_numbers = diff_line_numbers(session);

    // Build the rows for the side-by-side diff view.
    let max_lines =
        std::cmp::max(session.left_lines.len(), session.right_lines.len());
    if max_lines == 0 {
        let block = Block::default()
            .borders(Borders::ALL)
            .title(Span::styled(
                " Text Diff ",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ))
            .border_style(Style::default().fg(Color::Gray));
        let paragraph = Paragraph::new(Line::from("Empty file(s)."))
            .block(block)
            .alignment(Alignment::Center);
        frame.render_widget(paragraph, area);
        return;
    }

    let rows: Vec<_> = (0..max_lines)
        .map(|i| {
            let left_line = session.left_lines.get(i);
            let right_line = session.right_lines.get(i);

            let left_num = left_line.map(|l| l.number).unwrap_or(0);
            let right_num = right_line.map(|l| l.number).unwrap_or(0);

            let left_is_diff =
                left_line.is_some_and(|l| diff_numbers.contains(&l.number));
            let right_is_diff =
                right_line.is_some_and(|l| diff_numbers.contains(&l.number));

            // Format left side.
            let left_num_str = format!("{left_num:<6}");
            let left_content =
                left_line.as_ref().map(|l| l.content.as_str()).unwrap_or("");
            let left_importance = left_line
                .map(|l| importance_for_line(l))
                .unwrap_or(Importance::Ignored);

            // Format right side.
            let right_num_str = format!("{right_num:<6}");
            let right_content = right_line
                .as_ref()
                .map(|l| l.content.as_str())
                .unwrap_or("");
            let right_importance = right_line
                .map(|l| importance_for_line(l))
                .unwrap_or(Importance::Ignored);

            // Diff markers.
            let left_marker = if left_is_diff {
                "▶ "
            } else if left_line.is_none() {
                "  "
            } else {
                "  "
            };
            let right_marker = if right_is_diff {
                "▶ "
            } else if right_line.is_none() {
                "  "
            } else {
                "  "
            };

            let left_style = if left_is_diff {
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
            } else {
                grammar_style(left_importance)
            };
            let right_style = if right_is_diff {
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD)
            } else {
                grammar_style(right_importance)
            };

            Row::new(vec![
                Span::styled(
                    left_num_str,
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(left_marker, left_style),
                Span::styled(left_content, left_style),
                Span::styled(
                    right_num_str,
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(right_marker, right_style),
                Span::styled(right_content, right_style),
            ])
        })
        .collect();

    let header = Row::new(vec![
        Span::styled("Num", Style::default().add_modifier(Modifier::BOLD)),
        Span::styled("", Style::default()),
        Span::styled(
            session
                .left_path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default(),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("Num", Style::default().add_modifier(Modifier::BOLD)),
        Span::styled("", Style::default()),
        Span::styled(
            session
                .right_path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default(),
            Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD),
        ),
    ]);

    let widths = [
        Constraint::Length(7),
        Constraint::Length(2),
        Constraint::Min(20),
        Constraint::Length(7),
        Constraint::Length(2),
        Constraint::Min(20),
    ];

    let block = Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(
            " Text Diff ",
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

    frame.render_stateful_widget(
        table,
        area,
        &mut session.table_state.clone(),
    );
}

fn render_text_footer(frame: &mut Frame, area: Rect, session: &SessionView) {
    let diff_count = session
        .text_diff
        .as_ref()
        .map(|d| d.differences.len())
        .unwrap_or(0);

    let lines = vec![Line::from(format!(
        "Differences: {}  |  Left: {} lines  |  Right: {} lines",
        diff_count,
        session.left_lines.len(),
        session.right_lines.len(),
    ))];

    let help = Line::from(vec![
        Span::styled("j/k", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw(" navigate  "),
        Span::styled("n/N", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw(" next/prev diff  "),
        Span::styled("l/Tab", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw(" focus  "),
        Span::styled("Ctrl+W", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw(" close  "),
        Span::styled(
            "Ctrl+Tab",
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::raw(" tabs  "),
        Span::styled("q", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw(" quit"),
    ]);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Gray));
    let paragraph = Paragraph::new([lines, vec![help]].concat()).block(block);
    frame.render_widget(paragraph, area);
}

/// Return the highest importance token in a line.
fn importance_for_line(line: &LineInfo) -> Importance {
    let mut max_imp = Importance::Ignored;
    for token in &line.tokens {
        if token.importance > max_imp {
            max_imp = token.importance;
        }
    }
    max_imp
}

/// Return a text style based on the grammar importance of a token.
fn grammar_style(importance: Importance) -> Style {
    match importance {
        Importance::Code => Style::default(),
        Importance::Data => Style::default().fg(Color::Yellow),
        Importance::Comment => Style::default().fg(Color::DarkGray),
        Importance::Ignored => Style::default().fg(Color::DarkGray),
    }
}
