// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Key handling and formatting helpers for individual session views.
//!
//! Key handling returns [`SessionAction`] values that the App component acts
//! upon (e.g., creating new sessions for directory navigation). Rendering is
//! done by the ratatui-kit components in `main.rs`.

use cocomo_lib::{
    DirEntryStatus, Importance, LineInfo, SyncOperation, TextDifference,
};
use ratatui_kit::{
    crossterm::event::{KeyCode, KeyEvent},
    ratatui::style::{Color, Style},
};

use crate::app::{AppMode, Focus, SessionAction, SessionType, SessionView};

/// Handle key input for a session.
///
/// Returns a [`SessionAction`] if the key press requires the App component to
/// perform an async operation (e.g., opening a new directory comparison).
pub fn handle_session_key(
    session: &mut SessionView,
    key: KeyEvent,
) -> SessionAction {
    // Input modes (Filter, SaveSession, LoadSession) are handled by
    // ratatui-kit components (SearchInput / Input with input layers).  The
    // session key handler in DirCompareView returns Ignored for those
    // modes so the component layers receive keys; only Esc is handled as a
    // fallback to exit the mode.

    // Route to text-specific handler for text diff sessions.
    if session.session_type == SessionType::TextCompare {
        return handle_text_key(session, key);
    }

    match key.code {
        KeyCode::Char('q') | KeyCode::Char('Q') => {
            // The App component's global key handler handles quit.
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
        KeyCode::Char('C') => {
            session.compare_content = !session.compare_content;
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
            // active, but we dispatch it and the App component handles it).
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
    // The App component has access to the session paths and handles this
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
            if let Some(&target) = groups.iter().rev().find(|&&g| g > current)
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

/// Build a set of line numbers that are part of a difference.
pub fn diff_line_numbers(
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
pub fn current_diff_position(session: &SessionView) -> (usize, usize) {
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

/// Return the highest importance token in a line.
pub fn importance_for_line(line: &LineInfo) -> Importance {
    let mut max_imp = Importance::Ignored;
    for token in &line.tokens {
        if token.importance > max_imp {
            max_imp = token.importance;
        }
    }
    max_imp
}

/// Return a text style based on the grammar importance of a token.
pub fn grammar_style(importance: Importance) -> Style {
    match importance {
        Importance::Code => Style::default(),
        Importance::Data => Style::default().fg(Color::Yellow),
        Importance::Comment => Style::default().fg(Color::DarkGray),
        Importance::Ignored => Style::default().fg(Color::DarkGray),
    }
}
