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

use cocomo_lib::{
    DirEntryStatus, Importance, LineInfo, SyncOperation, TextDifference,
    TransferAction,
};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Row, Table},
};

use crate::app::{AppMode, Focus, SessionAction, SessionType, SessionView};

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
    // Render sync preview if active, otherwise the normal comparison.
    if session.sync_planned.is_some() {
        render_sync_preview(frame, chunks[1], session);
    } else {
        render_main(frame, chunks[1], session);
    }
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

/// Render the sync preview table showing planned transfer items.
fn render_sync_preview(frame: &mut Frame, area: Rect, session: &SessionView) {
    let Some(ref sync_result) = session.sync_planned else {
        return;
    };

    let executed = sync_result.transfer.is_some();

    // Build rows from planned transfer items.
    let rows: Vec<_> =
        sync_result
            .planned
            .iter()
            .map(|item| {
                let action_str = item.action.label().to_string();
                let dir_indicator = if item.is_dir { "📁 " } else { "  " };
                let path = &item.rel_path;

                // Color by action type.
                let action_color =
                    match item.action {
                        TransferAction::CopyLeft
                        | TransferAction::CopyRight => Color::Green,
                        TransferAction::CopyCenter => Color::Yellow,
                        TransferAction::DeleteLeft
                        | TransferAction::DeleteRight => Color::Red,
                        TransferAction::MoveLeft
                        | TransferAction::MoveRight => Color::Magenta,
                    };

                Row::new(vec![
                    Span::styled(
                        action_str,
                        Style::default()
                            .fg(action_color)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(format!("{dir_indicator}{path}")),
                ])
            })
            .collect();

    let header = Row::new(vec![
        Span::styled("Action", Style::default().add_modifier(Modifier::BOLD)),
        Span::styled("Path", Style::default().add_modifier(Modifier::BOLD)),
    ]);

    let widths = [Constraint::Length(30), Constraint::Min(20)];

    let title = if executed {
        " Sync Executed "
    } else {
        " Sync Preview (dry-run) "
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(
            title,
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
    } else if session.mode == AppMode::SaveSession {
        vec![
            Line::from(Span::styled(
                format!("Save session as: {}", session.filter_input),
                Style::default().fg(Color::Yellow),
            )),
            Line::from(Span::raw("Enter=save  Esc=cancel")),
        ]
    } else if session.mode == AppMode::LoadSession {
        vec![
            Line::from(Span::styled(
                format!("Load session: {}", session.filter_input),
                Style::default().fg(Color::Yellow),
            )),
            Line::from(Span::raw("j/k=select  Enter=load  Esc=cancel")),
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

    // Show different help text depending on context.
    let help = if session.sync_planned.is_some() {
        let count = session
            .sync_planned
            .as_ref()
            .map(|s| s.planned_count())
            .unwrap_or(0);
        let executed = session
            .sync_planned
            .as_ref()
            .map(|s| s.transfer.is_some())
            .unwrap_or(false);
        let op_label = session.sync_operation.label();
        let status = if executed {
            "Executed".to_string()
        } else {
            format!("Planned: {count} transfers")
        };
        Line::from(vec![
            Span::styled(
                format!("Sync: {op_label}  |  {status}"),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  |  "),
            Span::styled("j/k", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" navigate  "),
            Span::styled("Esc", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" cancel"),
        ])
    } else {
        Line::from(vec![
            Span::styled("j/k", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" navigate  "),
            Span::styled(
                "l/Tab",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(" focus  "),
            Span::styled("f", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" filter  "),
            Span::styled("F", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" hide same  "),
            Span::styled("S", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" files/struct  "),
            Span::styled("m/M", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" sync op  "),
            Span::styled("P", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" plan  "),
            Span::styled("e", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" execute  "),
            Span::styled("r", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" reload  "),
            Span::styled("s", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" save  "),
            Span::styled("O", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" load  "),
            Span::styled("↵", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" open dir  "),
            Span::styled("⌫", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" up  "),
            Span::styled(
                "Ctrl+W",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(" close  "),
            Span::styled(
                "Ctrl+Tab",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(" tabs  "),
            Span::styled("q", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" quit"),
        ])
    };

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
    // Handle input modes (filter, save session, load session).
    if session.mode == AppMode::Filter
        || session.mode == AppMode::SaveSession
        || session.mode == AppMode::LoadSession
    {
        return handle_filter_input(session, key);
    }

    // Route to text-specific handler for text diff sessions.
    if session.session_type == SessionType::TextCompare {
        return handle_text_key(session, key);
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
        KeyCode::Char('P') => {
            // Plan sync with the current operation (dry-run).
            SessionAction::PlanSync {
                operation: session.sync_operation,
            }
        }
        KeyCode::Char('m') => {
            // Cycle through sync operations.
            session.sync_operation = cycle_sync_op(&session.sync_operation, 1);
            SessionAction::None
        }
        KeyCode::Char('M') => {
            // Cycle through sync operations (reverse).
            session.sync_operation =
                cycle_sync_op(&session.sync_operation, -1);
            SessionAction::None
        }
        KeyCode::Char('e') => {
            // Execute the planned sync (only meaningful when preview is
            // active, but we dispatch it and the main loop handles it).
            SessionAction::ExecuteSync
        }
        KeyCode::Char('s') => {
            // Save session — enter input mode for the session name.
            session.mode = AppMode::SaveSession;
            session.filter_input.clear();
            SessionAction::None
        }
        KeyCode::Char('O') => {
            // List saved sessions for loading.
            session.mode = AppMode::LoadSession;
            SessionAction::ListSessions
        }
        KeyCode::Char('r') => SessionAction::Reload,
        KeyCode::Enter => handle_enter(session),
        KeyCode::Backspace => handle_backspace(session),
        KeyCode::Esc => {
            // If sync preview is active, cancel it. Otherwise, clear filter.
            if session.sync_planned.is_some() {
                session.sync_planned = None;
                session.table_state.select(Some(0));
            } else {
                session.active_filter = None;
                session.table_state.select(Some(0));
            }
            SessionAction::None
        }
        _ => SessionAction::None,
    }
}

fn handle_filter_input(
    session: &mut SessionView,
    key: KeyEvent,
) -> SessionAction {
    match key.code {
        KeyCode::Esc => {
            session.filter_input.clear();
            session.mode = AppMode::Normal;
            SessionAction::None
        }
        KeyCode::Enter => {
            // SaveSession mode — dispatch save action.
            if session.mode == AppMode::SaveSession {
                let name = if session.filter_input.is_empty() {
                    session.name.clone()
                } else {
                    session.filter_input.clone()
                };
                session.mode = AppMode::Normal;
                return SessionAction::SaveSession { name };
            }

            // LoadSession mode — load the first (or selected) session.
            if session.mode == AppMode::LoadSession {
                // filter_input contains comma-separated session names.
                // Build the path from the first entry.
                let session_dir = crate::app::default_session_dir();
                let entries: Vec<_> = session
                    .filter_input
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                let idx = session.table_state.selected().unwrap_or(0);
                if let Some(name) = entries.get(idx) {
                    let path = session_dir.join(format!("{name}.toml"));
                    session.mode = AppMode::Normal;
                    return SessionAction::LoadSession { path };
                }
                session.mode = AppMode::Normal;
                return SessionAction::None;
            }

            // Normal filter mode.
            if session.filter_input.is_empty() {
                session.active_filter = None;
            } else {
                session.active_filter = Some(session.filter_input.clone());
            }
            session.filter_input.clear();
            session.mode = AppMode::Normal;
            session.table_state.select(Some(0));
            SessionAction::None
        }
        KeyCode::Char('j')
        | KeyCode::Down
        | KeyCode::Char('k')
        | KeyCode::Up => {
            // In LoadSession mode, navigate through the session list.
            if session.mode == AppMode::LoadSession {
                let entries: Vec<_> = session
                    .filter_input
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                let delta = if matches!(
                    key.code,
                    KeyCode::Char('j') | KeyCode::Down
                ) {
                    1
                } else {
                    -1
                };
                if let Some(selected) = session.table_state.selected() {
                    let new_idx = if delta > 0 {
                        selected
                            .saturating_add(1)
                            .min(entries.len().saturating_sub(1))
                    } else {
                        selected.saturating_sub(1)
                    };
                    session.table_state.select(Some(new_idx));
                } else if !entries.is_empty() {
                    session.table_state.select(Some(0));
                }
            }
            SessionAction::None
        }
        KeyCode::Backspace => {
            session.filter_input.pop();
            SessionAction::None
        }
        KeyCode::Char(c) => {
            session.filter_input.push(c);
            SessionAction::None
        }
        _ => SessionAction::None,
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
// Text diff key handling
// ---------------------------------------------------------------------------

/// Cycle through sync operations.
fn cycle_sync_op(current: &SyncOperation, delta: isize) -> SyncOperation {
    let ops = [
        SyncOperation::MirrorLeft,
        SyncOperation::MirrorRight,
        SyncOperation::UpdateNewer,
        SyncOperation::UpdateBoth,
        SyncOperation::CopyLeft,
        SyncOperation::CopyRight,
        SyncOperation::CopyNewer,
        SyncOperation::DeleteOrphans,
    ];
    let len = ops.len() as isize;
    let idx = ops.iter().position(|x| x == current).unwrap_or(0) as isize;
    let new_idx = ((idx + delta) % len + len) % len;
    ops[new_idx as usize]
}

/// Handle keys specific to text diff sessions.
///
/// Returns [`SessionAction::None`] for all handled keys; text diff sessions
/// don't trigger any session-level actions.
pub fn handle_text_key(
    session: &mut SessionView,
    key: KeyEvent,
) -> SessionAction {
    let max_lines =
        std::cmp::max(session.left_lines.len(), session.right_lines.len());

    match key.code {
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
            if max_lines > 0 {
                session.table_state.select(Some(max_lines - 1));
            }
            SessionAction::None
        }
        KeyCode::Char('n') => {
            // Jump to the next difference group.
            let groups = diff_group_starts(session);
            if groups.is_empty() {
                return SessionAction::None;
            }

            let current = session.table_state.selected().unwrap_or(0);
            // Find the first group start > current.
            if let Some(&target) = groups.iter().find(|&&g| g > current) {
                session.table_state.select(Some(target));
            } else {
                // Wrap to the first group.
                session.table_state.select(Some(groups[0]));
            }
            SessionAction::None
        }
        KeyCode::Char('N') => {
            // Jump to the previous difference group.
            let groups = diff_group_starts(session);
            if groups.is_empty() {
                return SessionAction::None;
            }

            let current = session.table_state.selected().unwrap_or(0);
            // Find the last group start < current.
            if let Some(&target) =
                groups.iter().rev().filter(|&&g| g < current).next()
            {
                session.table_state.select(Some(target));
            } else {
                // Wrap to the last group.
                session.table_state.select(Some(*groups.last().unwrap()));
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
        _ => SessionAction::None,
    }
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

/// Return the first row index of each contiguous difference group.
///
/// Each element is the row index (0-based into the merged view) where a
/// difference group starts. These are the targets for n/N navigation.
fn diff_group_starts(session: &SessionView) -> Vec<usize> {
    let diff_numbers = diff_line_numbers(session);

    let max_lines =
        std::cmp::max(session.left_lines.len(), session.right_lines.len());

    // Find all rows that contain at least one diff line.
    let diff_rows: Vec<usize> = (0..max_lines)
        .filter(|&i| {
            let left_is_diff = session
                .left_lines
                .get(i)
                .is_some_and(|l| diff_numbers.contains(&l.number));
            let right_is_diff = session
                .right_lines
                .get(i)
                .is_some_and(|l| diff_numbers.contains(&l.number));
            left_is_diff || right_is_diff
        })
        .collect();

    // Group contiguous rows and return the first index of each group.
    let mut groups = Vec::new();
    let mut i = 0;
    while i < diff_rows.len() {
        groups.push(diff_rows[i]);
        while i + 1 < diff_rows.len() && diff_rows[i + 1] == diff_rows[i] + 1 {
            i += 1;
        }
        i += 1;
    }

    groups
}

/// Return the current diff group index (1-based) and total count.
///
/// Scans backward from the current selection to find which contiguous
/// difference group the cursor is on. Returns `(current, total)` where
/// `current` is 1-based. Returns `(0, total)` if no group is selected.
fn current_diff_position(session: &SessionView) -> (usize, usize) {
    let groups = diff_group_starts(session);
    let total = groups.len();
    if total == 0 {
        return (0, 0);
    }

    let selected = session.table_state.selected().unwrap_or(0);

    // Find the group whose start is <= selected.
    for (i, &start) in groups.iter().enumerate().rev() {
        if selected >= start {
            return (i + 1, total);
        }
    }

    // Selected is before the first group.
    (0, total)
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
    let (current, total) = current_diff_position(session);

    let position_str = if total > 0 {
        if current > 0 {
            format!("Diff {current}/{total}")
        } else {
            format!("Diff 0/{total} (no diff selected)")
        }
    } else {
        "No differences".to_string()
    };

    let lines = vec![Line::from(format!(
        "{position_str}  |  Left: {} lines  |  Right: {} lines",
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
