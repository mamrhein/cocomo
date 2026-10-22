// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Key handling for individual session views.
//!
//! Key handling returns [`SessionAction`] values that the controller acts
//! upon (e.g., creating new sessions for directory navigation). The key
//! handlers are pure with respect to the global state: they mutate only the
//! [`SessionView`] passed in and never touch `APP_STATE`. Rendering is done
//! by the ratatui-kit components in `ui/`.

use cocomo_lib::SyncOperation;
use ratatui_kit::crossterm::event::{KeyCode, KeyEvent};

use crate::{
    diff_model::diff_group_starts,
    state::{Focus, SessionAction, SessionMode, SessionType, SessionView},
};

/// Handle key input for a session.
///
/// Returns a [`SessionAction`] if the key press requires the App component to
/// perform an async operation (e.g., opening a new directory comparison).
pub(crate) fn handle_session_key(
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
            session.mode = SessionMode::Filter;
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
            session.mode = SessionMode::SaveSession;
            session.filter_input.clear();
            SessionAction::None
        }
        KeyCode::Char('O') => {
            // List saved sessions for loading.
            session.mode = SessionMode::LoadSession;
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
fn handle_text_key(session: &mut SessionView, key: KeyEvent) -> SessionAction {
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
