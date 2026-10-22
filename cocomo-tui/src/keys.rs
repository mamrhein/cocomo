// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Single source of truth for key bindings.
//!
//! The [`KEYMAP`] table maps (key, scope) pairs to [`Cmd`] values and to
//! their help-bar labels, so dispatch and documentation cannot drift apart.
//! Scopes partition key ownership: [`KeyScope::Global`] bindings belong to
//! the App shell, [`KeyScope::DirNormal`] and [`KeyScope::TextNormal`]
//! bindings to the two session views, and [`KeyScope::Editing`] is the
//! root-layer fallback while a text-entry mode is active.
//!
//! [`handle_session_key`] is pure with respect to the global state: it
//! mutates only the [`SessionView`] passed in and reports key ownership
//! through its return value — `None` means the key is unbound, so the
//! caller can return `EventResult::Ignored` instead of swallowing the key.
//! Rendering is done by the ratatui-kit components in `ui/`.

use cocomo_lib::SyncOperation;
use ratatui_kit::{
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
    ratatui::{
        style::{Modifier, Style},
        text::{Line, Span},
    },
};

use crate::{
    diff_model::diff_group_starts,
    state::{Focus, SessionAction, SessionMode, SessionType, SessionView},
};

/// Shorthand for an unmodified key binding.
const NONE: KeyModifiers = KeyModifiers::NONE;
/// Shorthand for a Ctrl binding.
const CTRL: KeyModifiers = KeyModifiers::CONTROL;

/// Which handler owns a binding.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum KeyScope {
    /// The App shell: tab management, menu navigation, quit.
    Global,
    /// A directory-comparison session in normal mode.
    DirNormal,
    /// A text-comparison session in normal mode.
    TextNormal,
    /// Any session while a text-entry mode is active. The input components
    /// capture keys in their own layers; this scope is only the root-layer
    /// fallback that cancels the mode.
    Editing,
}

/// The command a [`KeyBinding`] performs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Cmd {
    // -- Global scope: executed by the App shell. --
    /// Close the active tab (quit when it was the last one).
    CloseTab,
    /// Switch to the next tab, or the previous one with Shift.
    SwitchTab,
    /// Select the previous menu item.
    MenuPrev,
    /// Select the next menu item.
    MenuNext,
    /// Quit the application.
    Quit,
    // -- Session scopes: executed by `execute`. --
    /// Move the selection one row down.
    NextRow,
    /// Move the selection one row up.
    PrevRow,
    /// Select the first row.
    FirstRow,
    /// Select the last row.
    LastRow,
    /// Toggle keyboard focus between the left and right pane.
    ToggleFocus,
    /// Enter filter mode.
    StartFilter,
    /// Toggle hiding identical entries.
    ToggleHideSame,
    /// Toggle content comparison.
    ToggleCompareContent,
    /// Plan a sync (dry-run) with the current operation.
    PlanSync,
    /// Cycle the sync operation forward.
    CycleSyncOpForward,
    /// Cycle the sync operation backward.
    CycleSyncOpBackward,
    /// Execute the planned sync.
    ExecuteSync,
    /// Enter save-session mode.
    StartSaveSession,
    /// Enter load-session mode and list saved sessions.
    StartLoadSession,
    /// Re-run the current comparison.
    Reload,
    /// Open the selected entry in a new session.
    OpenEntry,
    /// Open the parent directories in a new session.
    GoUp,
    /// Cancel the sync preview, else clear the filter.
    Cancel,
    /// Jump to the next difference group.
    NextDiff,
    /// Jump to the previous difference group.
    PrevDiff,
    /// Leave a text-entry mode without applying it.
    CancelEdit,
}

/// A single key binding: which key in which scope runs which command, and
/// how it is documented in the help bar.
#[derive(Clone, Copy, Debug)]
pub(crate) struct KeyBinding {
    /// Key code that triggers this binding.
    key: KeyCode,
    /// Modifiers that must be present. A `NONE` binding never fires for
    /// Ctrl or Alt combinations.
    modifiers: KeyModifiers,
    /// Handler that owns this binding.
    scope: KeyScope,
    /// Command to run.
    cmd: Cmd,
    /// Help-bar entry `(keys, description)`; `None` hides the binding
    /// (e.g. the `Down` alias of `j`).
    help: Option<(&'static str, &'static str)>,
}

/// The complete keymap. Array order determines help-bar order; bindings
/// without a help label are silent aliases.
const KEYMAP: &[KeyBinding] = &[
    // -- Directory comparison, normal mode. --
    KeyBinding {
        key: KeyCode::Char('j'),
        modifiers: NONE,
        scope: KeyScope::DirNormal,
        cmd: Cmd::NextRow,
        help: Some(("j/k", "navigate")),
    },
    KeyBinding {
        key: KeyCode::Down,
        modifiers: NONE,
        scope: KeyScope::DirNormal,
        cmd: Cmd::NextRow,
        help: None,
    },
    KeyBinding {
        key: KeyCode::Char('k'),
        modifiers: NONE,
        scope: KeyScope::DirNormal,
        cmd: Cmd::PrevRow,
        help: None,
    },
    KeyBinding {
        key: KeyCode::Up,
        modifiers: NONE,
        scope: KeyScope::DirNormal,
        cmd: Cmd::PrevRow,
        help: None,
    },
    KeyBinding {
        key: KeyCode::Char('g'),
        modifiers: NONE,
        scope: KeyScope::DirNormal,
        cmd: Cmd::FirstRow,
        help: Some(("g/G", "top/bottom")),
    },
    KeyBinding {
        key: KeyCode::Char('G'),
        modifiers: NONE,
        scope: KeyScope::DirNormal,
        cmd: Cmd::LastRow,
        help: None,
    },
    KeyBinding {
        key: KeyCode::Char('l'),
        modifiers: NONE,
        scope: KeyScope::DirNormal,
        cmd: Cmd::ToggleFocus,
        help: Some(("l/Tab", "focus")),
    },
    KeyBinding {
        key: KeyCode::Tab,
        modifiers: NONE,
        scope: KeyScope::DirNormal,
        cmd: Cmd::ToggleFocus,
        help: None,
    },
    KeyBinding {
        key: KeyCode::Char('f'),
        modifiers: NONE,
        scope: KeyScope::DirNormal,
        cmd: Cmd::StartFilter,
        help: Some(("f", "filter")),
    },
    KeyBinding {
        key: KeyCode::Char('F'),
        modifiers: NONE,
        scope: KeyScope::DirNormal,
        cmd: Cmd::ToggleHideSame,
        help: Some(("F", "hide same")),
    },
    KeyBinding {
        key: KeyCode::Char('C'),
        modifiers: NONE,
        scope: KeyScope::DirNormal,
        cmd: Cmd::ToggleCompareContent,
        help: Some(("C", "cont. on/off")),
    },
    KeyBinding {
        key: KeyCode::Char('m'),
        modifiers: NONE,
        scope: KeyScope::DirNormal,
        cmd: Cmd::CycleSyncOpForward,
        help: Some(("m/M", "sync op")),
    },
    KeyBinding {
        key: KeyCode::Char('M'),
        modifiers: NONE,
        scope: KeyScope::DirNormal,
        cmd: Cmd::CycleSyncOpBackward,
        help: None,
    },
    KeyBinding {
        key: KeyCode::Char('P'),
        modifiers: NONE,
        scope: KeyScope::DirNormal,
        cmd: Cmd::PlanSync,
        help: Some(("P", "plan")),
    },
    KeyBinding {
        key: KeyCode::Char('e'),
        modifiers: NONE,
        scope: KeyScope::DirNormal,
        cmd: Cmd::ExecuteSync,
        help: Some(("e", "execute")),
    },
    KeyBinding {
        key: KeyCode::Char('r'),
        modifiers: NONE,
        scope: KeyScope::DirNormal,
        cmd: Cmd::Reload,
        help: Some(("r", "reload")),
    },
    KeyBinding {
        key: KeyCode::Char('s'),
        modifiers: NONE,
        scope: KeyScope::DirNormal,
        cmd: Cmd::StartSaveSession,
        help: Some(("s", "save")),
    },
    KeyBinding {
        key: KeyCode::Char('O'),
        modifiers: NONE,
        scope: KeyScope::DirNormal,
        cmd: Cmd::StartLoadSession,
        help: Some(("O", "load")),
    },
    KeyBinding {
        key: KeyCode::Enter,
        modifiers: NONE,
        scope: KeyScope::DirNormal,
        cmd: Cmd::OpenEntry,
        help: Some(("\u{21A5}", "open dir")),
    },
    KeyBinding {
        key: KeyCode::Backspace,
        modifiers: NONE,
        scope: KeyScope::DirNormal,
        cmd: Cmd::GoUp,
        help: Some(("\u{232B}", "up")),
    },
    KeyBinding {
        key: KeyCode::Esc,
        modifiers: NONE,
        scope: KeyScope::DirNormal,
        cmd: Cmd::Cancel,
        help: Some(("Esc", "cancel")),
    },
    // -- Text comparison, normal mode. --
    KeyBinding {
        key: KeyCode::Char('j'),
        modifiers: NONE,
        scope: KeyScope::TextNormal,
        cmd: Cmd::NextRow,
        help: Some(("j/k", "navigate")),
    },
    KeyBinding {
        key: KeyCode::Down,
        modifiers: NONE,
        scope: KeyScope::TextNormal,
        cmd: Cmd::NextRow,
        help: None,
    },
    KeyBinding {
        key: KeyCode::Char('k'),
        modifiers: NONE,
        scope: KeyScope::TextNormal,
        cmd: Cmd::PrevRow,
        help: None,
    },
    KeyBinding {
        key: KeyCode::Up,
        modifiers: NONE,
        scope: KeyScope::TextNormal,
        cmd: Cmd::PrevRow,
        help: None,
    },
    KeyBinding {
        key: KeyCode::Char('g'),
        modifiers: NONE,
        scope: KeyScope::TextNormal,
        cmd: Cmd::FirstRow,
        help: Some(("g/G", "top/bottom")),
    },
    KeyBinding {
        key: KeyCode::Char('G'),
        modifiers: NONE,
        scope: KeyScope::TextNormal,
        cmd: Cmd::LastRow,
        help: None,
    },
    KeyBinding {
        key: KeyCode::Char('n'),
        modifiers: NONE,
        scope: KeyScope::TextNormal,
        cmd: Cmd::NextDiff,
        help: Some(("n/N", "next/prev diff")),
    },
    KeyBinding {
        key: KeyCode::Char('N'),
        modifiers: NONE,
        scope: KeyScope::TextNormal,
        cmd: Cmd::PrevDiff,
        help: None,
    },
    KeyBinding {
        key: KeyCode::Char('l'),
        modifiers: NONE,
        scope: KeyScope::TextNormal,
        cmd: Cmd::ToggleFocus,
        help: Some(("l/Tab", "focus")),
    },
    KeyBinding {
        key: KeyCode::Tab,
        modifiers: NONE,
        scope: KeyScope::TextNormal,
        cmd: Cmd::ToggleFocus,
        help: None,
    },
    // -- Text-entry modes (root-layer fallback; the input layers normally
    //    capture keys exclusively). --
    KeyBinding {
        key: KeyCode::Esc,
        modifiers: NONE,
        scope: KeyScope::Editing,
        cmd: Cmd::CancelEdit,
        help: None,
    },
    KeyBinding {
        key: KeyCode::Char('q'),
        modifiers: NONE,
        scope: KeyScope::Editing,
        cmd: Cmd::CancelEdit,
        help: None,
    },
    // -- App shell. --
    KeyBinding {
        key: KeyCode::Char('w'),
        modifiers: CTRL,
        scope: KeyScope::Global,
        cmd: Cmd::CloseTab,
        help: Some(("Ctrl+W", "close")),
    },
    KeyBinding {
        key: KeyCode::Tab,
        modifiers: CTRL,
        scope: KeyScope::Global,
        cmd: Cmd::SwitchTab,
        help: Some(("Ctrl+Tab", "tabs")),
    },
    KeyBinding {
        key: KeyCode::Left,
        modifiers: NONE,
        scope: KeyScope::Global,
        cmd: Cmd::MenuPrev,
        help: Some(("\u{2190}/\u{2192}", "menu")),
    },
    KeyBinding {
        key: KeyCode::Right,
        modifiers: NONE,
        scope: KeyScope::Global,
        cmd: Cmd::MenuNext,
        help: None,
    },
    KeyBinding {
        key: KeyCode::Char('q'),
        modifiers: NONE,
        scope: KeyScope::Global,
        cmd: Cmd::Quit,
        help: Some(("q", "quit")),
    },
    KeyBinding {
        key: KeyCode::Char('Q'),
        modifiers: NONE,
        scope: KeyScope::Global,
        cmd: Cmd::Quit,
        help: None,
    },
];

/// Return the command bound to `key` in `scope`, if any.
///
/// A binding matches when the key code is equal, the required modifiers are
/// present, and — for unmodified bindings — no Ctrl/Alt modifier is set, so
/// `Ctrl+Tab` never fires the plain `Tab` binding.
pub(crate) fn lookup(scope: KeyScope, key: KeyEvent) -> Option<Cmd> {
    KEYMAP
        .iter()
        .find(|b| {
            b.scope == scope
                && b.key == key.code
                && key.modifiers.contains(b.modifiers)
                && (b.modifiers != NONE
                    || !key.modifiers.intersects(CTRL | KeyModifiers::ALT))
        })
        .map(|b| b.cmd)
}

/// Build the help bar line for the given scopes from the keymap.
///
/// Bindings appear in [`KEYMAP`] order; aliases without a help label are
/// skipped. Because the labels come from the same table that drives
/// dispatch, the help bar cannot drift from the actual bindings.
pub(crate) fn help_line(scopes: &[KeyScope]) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    for binding in KEYMAP {
        if let Some((keys, description)) = binding.help {
            if !scopes.contains(&binding.scope) {
                continue;
            }
            if !spans.is_empty() {
                spans.push(Span::raw("  "));
            }
            spans.push(Span::styled(
                keys,
                Style::new().add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::raw(format!(" {description}")));
        }
    }
    Line::from(spans)
}

/// Handle key input for a session.
///
/// Returns `None` when the key is unbound in the session's current scope,
/// so the caller can report `EventResult::Ignored` and let sibling handlers
/// see the key. Otherwise returns the [`SessionAction`] the controller must
/// dispatch.
pub(crate) fn handle_session_key(
    session: &mut SessionView,
    key: KeyEvent,
) -> Option<SessionAction> {
    let scope = if session.mode.is_editing() {
        KeyScope::Editing
    } else {
        match session.session_type {
            SessionType::DirCompare => KeyScope::DirNormal,
            SessionType::TextCompare => KeyScope::TextNormal,
        }
    };
    let cmd = lookup(scope, key)?;
    Some(execute(cmd, session))
}

/// Run a session-scoped [`Cmd`], mutating the session and returning the
/// action the controller must dispatch.
fn execute(cmd: Cmd, session: &mut SessionView) -> SessionAction {
    match cmd {
        Cmd::NextRow => {
            navigate(session, 1);
            SessionAction::None
        }
        Cmd::PrevRow => {
            navigate(session, -1);
            SessionAction::None
        }
        Cmd::FirstRow => {
            session.table_state.select(Some(0));
            SessionAction::None
        }
        Cmd::LastRow => {
            let count = session.row_count();
            if count > 0 {
                session.table_state.select(Some(count - 1));
            }
            SessionAction::None
        }
        Cmd::ToggleFocus => {
            session.focus = match session.focus {
                Focus::Left => Focus::Right,
                Focus::Right => Focus::Left,
            };
            SessionAction::None
        }
        Cmd::StartFilter => {
            session.mode = SessionMode::Filter;
            session.filter_input.clear();
            SessionAction::None
        }
        Cmd::ToggleHideSame => {
            session.hide_same = !session.hide_same;
            session.table_state.select(Some(0));
            SessionAction::None
        }
        Cmd::ToggleCompareContent => {
            session.compare_content = !session.compare_content;
            SessionAction::None
        }
        Cmd::PlanSync => {
            // Plan sync with the current operation (dry-run).
            SessionAction::PlanSync {
                operation: session.sync_operation,
            }
        }
        Cmd::CycleSyncOpForward => {
            session.sync_operation = cycle_sync_op(&session.sync_operation, 1);
            SessionAction::None
        }
        Cmd::CycleSyncOpBackward => {
            session.sync_operation =
                cycle_sync_op(&session.sync_operation, -1);
            SessionAction::None
        }
        Cmd::ExecuteSync => {
            // Execute the planned sync (only meaningful when a preview is
            // active, but we dispatch it and the controller handles it).
            SessionAction::ExecuteSync
        }
        Cmd::StartSaveSession => {
            // Enter input mode for the session name.
            session.mode = SessionMode::SaveSession;
            session.filter_input.clear();
            SessionAction::None
        }
        Cmd::StartLoadSession => {
            // Enter load mode and list saved sessions for it.
            session.mode = SessionMode::LoadSession;
            SessionAction::ListSessions
        }
        Cmd::Reload => SessionAction::Reload,
        Cmd::OpenEntry => handle_enter(session),
        Cmd::GoUp => SessionAction::GoUp,
        Cmd::Cancel => {
            // If sync preview is active, cancel it. Otherwise, clear the
            // filter.
            if session.sync_planned.is_some() {
                session.sync_planned = None;
            } else {
                session.active_filter = None;
            }
            session.table_state.select(Some(0));
            SessionAction::None
        }
        Cmd::NextDiff => {
            jump_diff(session, 1);
            SessionAction::None
        }
        Cmd::PrevDiff => {
            jump_diff(session, -1);
            SessionAction::None
        }
        Cmd::CancelEdit => {
            session.mode = SessionMode::Normal;
            session.filter_input.clear();
            SessionAction::None
        }
        // Global commands are dispatched by the App shell, never here.
        Cmd::CloseTab
        | Cmd::SwitchTab
        | Cmd::MenuPrev
        | Cmd::MenuNext
        | Cmd::Quit => SessionAction::None,
    }
}

/// Move the selection by `delta` rows, clamped to the session's row count.
fn navigate(session: &mut SessionView, delta: isize) {
    let rows = session.row_count();
    if delta > 0 {
        if let Some(selected) = session.table_state.selected() {
            if selected + 1 < rows {
                session.table_state.select(Some(selected + 1));
            }
        } else if rows > 0 {
            session.table_state.select(Some(0));
        }
    } else if let Some(selected) =
        session.table_state.selected().filter(|s| *s > 0)
    {
        session.table_state.select(Some(selected - 1));
    }
}

/// Jump to the next (`delta > 0`) or previous difference group.
///
/// Wraps around at both ends: forward wraps to the first group, backward
/// wraps to the last.
fn jump_diff(session: &mut SessionView, delta: isize) {
    let groups = diff_group_starts(session);
    if groups.is_empty() {
        return;
    }

    let current = session.table_state.selected().unwrap_or(0);
    let target = if delta > 0 {
        // First group start after the cursor.
        groups.iter().copied().find(|&g| g > current)
    } else {
        // Last group start before the cursor.
        groups.iter().rev().copied().find(|&g| g < current)
    };

    // Wrap around at the ends: forward to the first group, backward to the
    // last.
    let fallback = if delta > 0 {
        groups[0]
    } else {
        *groups.last().expect("groups was checked to be non-empty")
    };
    session.table_state.select(Some(target.unwrap_or(fallback)));
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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use cocomo_lib::{
        DirComparison, DirEntry, DirEntryStatus, Grammar, LineInfo, TextDiff,
        TextDifference,
    };
    use ratatui_kit::crossterm::event::KeyEvent;

    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, NONE)
    }

    /// Text session with 10 merged rows and diff groups at rows 2 and 6.
    fn text_session() -> SessionView {
        let left_lines: Vec<LineInfo> = (1..=10)
            .map(|n| LineInfo::new(n, format!("left {n}")))
            .collect();
        let right_lines: Vec<LineInfo> = (1..=10)
            .map(|n| LineInfo::new(n, format!("right {n}")))
            .collect();
        let diff = TextDiff {
            differences: vec![
                TextDifference::LineDifferent(
                    LineInfo::new(3, "a".into()),
                    LineInfo::new(3, "b".into()),
                ),
                TextDifference::LineDifferent(
                    LineInfo::new(7, "c".into()),
                    LineInfo::new(7, "d".into()),
                ),
            ],
            ..Default::default()
        };
        SessionView::new_text_compare(
            PathBuf::from("/left"),
            PathBuf::from("/right"),
            left_lines,
            right_lines,
            diff,
            Grammar::plain_text(),
        )
    }

    /// Dir session with three entries.
    fn dir_session() -> SessionView {
        let entries = (0..3)
            .map(|i| DirEntry {
                name: format!("entry{i}"),
                status: DirEntryStatus::Different,
                left: None,
                right: None,
                center: None,
                sub_entries: None,
            })
            .collect();
        let mut session = SessionView::new_dir_compare(
            PathBuf::from("/left"),
            PathBuf::from("/right"),
            false,
        );
        session.comparison = Some(DirComparison {
            entries,
            ..Default::default()
        });
        session
    }

    fn press(
        session: &mut SessionView,
        code: KeyCode,
    ) -> Option<SessionAction> {
        handle_session_key(session, key(code))
    }

    #[test]
    fn row_count_matches_session_type() {
        assert_eq!(dir_session().row_count(), 3);
        assert_eq!(text_session().row_count(), 10);
    }

    #[test]
    fn j_k_navigate_text_rows() {
        // Regression: j/k used to derive the row count from
        // `filtered_entries()`, which is empty for text sessions.
        let mut session = text_session();
        assert!(press(&mut session, KeyCode::Char('j')).is_some());
        assert_eq!(session.table_state.selected(), Some(0));
        press(&mut session, KeyCode::Char('j'));
        assert_eq!(session.table_state.selected(), Some(1));
        press(&mut session, KeyCode::Char('k'));
        assert_eq!(session.table_state.selected(), Some(0));
        // Clamped at the top.
        press(&mut session, KeyCode::Char('k'));
        assert_eq!(session.table_state.selected(), Some(0));
    }

    #[test]
    fn shift_g_jumps_to_bounds() {
        let mut session = text_session();
        press(&mut session, KeyCode::Char('G'));
        assert_eq!(session.table_state.selected(), Some(9));
        press(&mut session, KeyCode::Char('j'));
        // Clamped at the bottom.
        assert_eq!(session.table_state.selected(), Some(9));
        press(&mut session, KeyCode::Char('g'));
        assert_eq!(session.table_state.selected(), Some(0));
    }

    #[test]
    fn n_jumps_to_next_diff_and_wraps() {
        let mut session = text_session();
        session.table_state.select(Some(0));
        press(&mut session, KeyCode::Char('n'));
        assert_eq!(session.table_state.selected(), Some(2));
        press(&mut session, KeyCode::Char('n'));
        assert_eq!(session.table_state.selected(), Some(6));
        // Wrap to the first group.
        press(&mut session, KeyCode::Char('n'));
        assert_eq!(session.table_state.selected(), Some(2));
    }

    #[test]
    fn shift_n_jumps_to_prev_diff_and_wraps() {
        // Regression: N used to search for groups *after* the cursor, so it
        // jumped forward instead of backward.
        let mut session = text_session();
        session.table_state.select(Some(9));
        press(&mut session, KeyCode::Char('N'));
        assert_eq!(session.table_state.selected(), Some(6));
        press(&mut session, KeyCode::Char('N'));
        assert_eq!(session.table_state.selected(), Some(2));
        // Wrap to the last group.
        press(&mut session, KeyCode::Char('N'));
        assert_eq!(session.table_state.selected(), Some(6));
    }

    #[test]
    fn unbound_keys_are_not_consumed() {
        let mut session = dir_session();
        // `x` is bound nowhere.
        assert_eq!(press(&mut session, KeyCode::Char('x')), None);
        // `q` is owned by the App shell, not the session handler.
        assert_eq!(press(&mut session, KeyCode::Char('q')), None);
        // Dir-only keys are unbound in a text session.
        let mut session = text_session();
        assert_eq!(press(&mut session, KeyCode::Char('f')), None);
    }

    #[test]
    fn editing_mode_esc_cancels_and_other_keys_pass_through() {
        let mut session = dir_session();
        session.mode = SessionMode::Filter;
        session.filter_input = "partial".into();
        let action = press(&mut session, KeyCode::Esc);
        assert_eq!(action, Some(SessionAction::None));
        assert_eq!(session.mode, SessionMode::Normal);
        assert!(session.filter_input.is_empty());

        // While editing, printable keys are not owned by this handler.
        session.mode = SessionMode::Filter;
        assert_eq!(press(&mut session, KeyCode::Char('x')), None);
        // `q` cancels the mode instead of quitting.
        assert!(press(&mut session, KeyCode::Char('q')).is_some());
        assert_eq!(session.mode, SessionMode::Normal);
    }

    #[test]
    fn ctrl_tab_is_not_the_plain_tab_binding() {
        // Ctrl+Tab belongs to the shell; plain Tab toggles focus.
        let ctrl_tab = KeyEvent::new(KeyCode::Tab, CTRL | KeyModifiers::SHIFT);
        assert_eq!(lookup(KeyScope::Global, ctrl_tab), Some(Cmd::SwitchTab));
        assert_eq!(lookup(KeyScope::DirNormal, ctrl_tab), None);
        assert_eq!(
            lookup(KeyScope::DirNormal, key(KeyCode::Tab)),
            Some(Cmd::ToggleFocus)
        );
    }

    #[test]
    fn help_line_contains_all_labelled_bindings_of_its_scopes() {
        let line = help_line(&[KeyScope::TextNormal, KeyScope::Global]);
        let text: String =
            line.spans.iter().map(|s| s.content.as_ref()).collect();
        for label in ["j/k", "g/G", "n/N", "l/Tab", "Ctrl+W", "Ctrl+Tab", "q"]
        {
            assert!(text.contains(label), "missing `{label}` in `{text}`");
        }
        // Dir-only labels must not appear in the text help bar.
        assert!(!text.contains("filter"));
    }
}
