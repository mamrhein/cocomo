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
//! bindings to the two session views, [`KeyScope::Dialog`] to the open
//! transfer dialog, and [`KeyScope::Editing`] is the root-layer fallback
//! while a text-entry mode is active.
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
    state::{
        SessionAction, SessionContent, SessionMode, SessionView,
        TransferChoice, TransferOp,
    },
};

/// Shorthand for an unmodified key binding.
const NONE: KeyModifiers = KeyModifiers::NONE;
/// Shorthand for a Ctrl binding.
const CTRL: KeyModifiers = KeyModifiers::CONTROL;

/// Which handler owns a binding.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum KeyScope {
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
    /// The transfer dialog is open. Its accelerators live here so they stay
    /// documented in the table; navigation and `Enter` are owned by the
    /// dialog's `Select` component and deliberately not listed.
    Dialog,
}

/// The command a [`KeyBinding`] performs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Cmd {
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
    /// Toggle the bottom help bar.
    ToggleHelp,
    // -- Session scopes: executed by `execute`. --
    /// Move the selection one row down.
    NextRow,
    /// Move the selection one row up.
    PrevRow,
    /// Select the first row.
    FirstRow,
    /// Select the last row.
    LastRow,
    /// Enter filter mode.
    StartFilter,
    /// Toggle hiding identical entries.
    ToggleHideSame,
    /// Toggle content comparison.
    ToggleCompareContent,
    /// Plan the sync (dry-run), then confirm its execution (`S`).
    Sync,
    /// Cycle the sync operation forward.
    CycleSyncOpForward,
    /// Cycle the sync operation backward.
    CycleSyncOpBackward,
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
    /// Expand the selected directory in place (`+`).
    ExpandDir,
    /// Collapse the selected directory in place (`-`).
    CollapseDir,
    /// Open the copy dialog for the selected entry (`c`).
    CopyDialog,
    /// Open the delete dialog for the selected entry (`d`).
    DeleteDialog,
    /// Confirm the given option of the open transfer dialog.
    DialogPick(TransferChoice),
    /// Close the transfer dialog without transferring.
    DialogAbort,
    /// Enter search mode (`/`).
    StartSearch,
    /// Enter command mode (`:`).
    StartCommand,
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
    /// (e.g. the `Down` alias of `j`, or the dialog accelerators, which the
    /// dialog labels itself).
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
        key: KeyCode::Char('n'),
        modifiers: NONE,
        scope: KeyScope::DirNormal,
        cmd: Cmd::NextDiff,
        help: Some(("n/N", "next/prev diff")),
    },
    KeyBinding {
        key: KeyCode::Char('N'),
        modifiers: NONE,
        scope: KeyScope::DirNormal,
        cmd: Cmd::PrevDiff,
        help: None,
    },
    KeyBinding {
        key: KeyCode::Char('+'),
        modifiers: NONE,
        scope: KeyScope::DirNormal,
        cmd: Cmd::ExpandDir,
        help: Some(("+", "expand")),
    },
    KeyBinding {
        key: KeyCode::Char('-'),
        modifiers: NONE,
        scope: KeyScope::DirNormal,
        cmd: Cmd::CollapseDir,
        help: Some(("-", "collapse")),
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
        // `S` runs the whole sync flow: the controller plans the sync and
        // opens the confirmation modal once the plan is ready.
        key: KeyCode::Char('S'),
        modifiers: NONE,
        scope: KeyScope::DirNormal,
        cmd: Cmd::Sync,
        help: Some(("S", "sync")),
    },
    KeyBinding {
        key: KeyCode::Char('c'),
        modifiers: NONE,
        scope: KeyScope::DirNormal,
        cmd: Cmd::CopyDialog,
        help: Some(("c", "copy")),
    },
    KeyBinding {
        key: KeyCode::Char('d'),
        modifiers: NONE,
        scope: KeyScope::DirNormal,
        cmd: Cmd::DeleteDialog,
        help: Some(("d", "delete")),
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
        key: KeyCode::Char('/'),
        modifiers: NONE,
        scope: KeyScope::DirNormal,
        cmd: Cmd::StartSearch,
        help: Some(("/", "search")),
    },
    KeyBinding {
        key: KeyCode::Char(':'),
        modifiers: NONE,
        scope: KeyScope::DirNormal,
        cmd: Cmd::StartCommand,
        help: Some((":", "command")),
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
        key: KeyCode::Char('c'),
        modifiers: NONE,
        scope: KeyScope::TextNormal,
        cmd: Cmd::CopyDialog,
        help: Some(("c", "copy")),
    },
    KeyBinding {
        key: KeyCode::Char('d'),
        modifiers: NONE,
        scope: KeyScope::TextNormal,
        cmd: Cmd::DeleteDialog,
        help: Some(("d", "delete")),
    },
    KeyBinding {
        key: KeyCode::Char('/'),
        modifiers: NONE,
        scope: KeyScope::TextNormal,
        cmd: Cmd::StartSearch,
        help: Some(("/", "search")),
    },
    KeyBinding {
        key: KeyCode::Char(':'),
        modifiers: NONE,
        scope: KeyScope::TextNormal,
        cmd: Cmd::StartCommand,
        help: Some((":", "command")),
    },
    // -- Transfer dialog (its own input layer; the `Select` inside owns
    //    j/k/Enter, so only the accelerators are listed here). --
    KeyBinding {
        key: KeyCode::Esc,
        modifiers: NONE,
        scope: KeyScope::Dialog,
        cmd: Cmd::DialogAbort,
        help: None,
    },
    KeyBinding {
        key: KeyCode::Char('a'),
        modifiers: NONE,
        scope: KeyScope::Dialog,
        cmd: Cmd::DialogAbort,
        help: None,
    },
    KeyBinding {
        key: KeyCode::Char('l'),
        modifiers: NONE,
        scope: KeyScope::Dialog,
        cmd: Cmd::DialogPick(TransferChoice::Left),
        help: None,
    },
    KeyBinding {
        key: KeyCode::Char('r'),
        modifiers: NONE,
        scope: KeyScope::Dialog,
        cmd: Cmd::DialogPick(TransferChoice::Right),
        help: None,
    },
    KeyBinding {
        key: KeyCode::Char('b'),
        modifiers: NONE,
        scope: KeyScope::Dialog,
        cmd: Cmd::DialogPick(TransferChoice::Both),
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
        modifiers: NONE,
        scope: KeyScope::Global,
        cmd: Cmd::SwitchTab,
        help: Some(("Tab", "tabs")),
    },
    KeyBinding {
        key: KeyCode::Tab,
        modifiers: CTRL,
        scope: KeyScope::Global,
        cmd: Cmd::SwitchTab,
        help: None,
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
    KeyBinding {
        key: KeyCode::Char('?'),
        modifiers: NONE,
        scope: KeyScope::Global,
        cmd: Cmd::ToggleHelp,
        help: Some(("?", "help")),
    },
    KeyBinding {
        key: KeyCode::F(1),
        modifiers: NONE,
        scope: KeyScope::Global,
        cmd: Cmd::ToggleHelp,
        help: None,
    },
];

/// Return the command bound to `key` in `scope`, if any.
///
/// A binding matches when the key code is equal, the required modifiers are
/// present, and — for unmodified bindings — no Ctrl/Alt modifier is set, so
/// `Ctrl+Tab` never fires the plain `Tab` binding.
pub fn lookup(scope: KeyScope, key: KeyEvent) -> Option<Cmd> {
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

/// Build the help bar lines for the given scopes from the keymap.
///
/// Bindings appear in [`KEYMAP`] order; aliases without a help label are
/// skipped. Items are packed greedily into lines of at most `width` cells
/// (an item wider than `width` gets a line of its own instead of being
/// split), so every labelled binding stays visible at any terminal width.
/// Because the labels come from the same table that drives dispatch, the
/// help bar cannot drift from the actual bindings.
pub fn help_lines(scopes: &[KeyScope], width: u16) -> Vec<Line<'static>> {
    /// Cells separating two items on the same line.
    const SEP_WIDTH: usize = 2;

    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut line_width = 0usize;
    for binding in KEYMAP {
        let Some((keys, description)) = binding.help else {
            continue;
        };
        if !scopes.contains(&binding.scope) {
            continue;
        }
        let key_span =
            Span::styled(keys, Style::new().add_modifier(Modifier::BOLD));
        let desc_span = Span::raw(format!(" {description}"));
        let item_width = key_span.width() + desc_span.width();
        if !spans.is_empty()
            && line_width + SEP_WIDTH + item_width > usize::from(width)
        {
            lines.push(Line::from(std::mem::take(&mut spans)));
            line_width = 0;
        }
        if !spans.is_empty() {
            spans.push(Span::raw("  "));
            line_width += SEP_WIDTH;
        }
        line_width += item_width;
        spans.push(key_span);
        spans.push(desc_span);
    }
    if !spans.is_empty() {
        lines.push(Line::from(spans));
    }
    lines
}

/// Handle key input for a session.
///
/// Returns `None` when the key is unbound in the session's current scope,
/// so the caller can report `EventResult::Ignored` and let sibling handlers
/// see the key. Otherwise returns the [`SessionAction`] the controller must
/// dispatch.
///
/// While the transfer dialog is open its scope owns the keys, which keeps
/// the dialog accelerators in the same table as every other binding even
/// though the dialog renders in its own input layer.
pub fn handle_session_key(
    session: &mut SessionView,
    key: KeyEvent,
) -> Option<SessionAction> {
    let scope = if session.ui.mode.is_editing() {
        KeyScope::Editing
    } else if matches!(session.ui.mode, SessionMode::Transfer { .. }) {
        KeyScope::Dialog
    } else if matches!(session.ui.mode, SessionMode::SyncConfirm) {
        // The sync confirmation modal owns every key while it is open;
        // its exclusive input layer normally blocks this handler already,
        // and claiming nothing here is the belt-and-braces counterpart.
        return None;
    } else {
        match &session.content {
            SessionContent::Dir(_) => KeyScope::DirNormal,
            SessionContent::Text(_) => KeyScope::TextNormal,
        }
    };
    Some(execute(lookup(scope, key)?, session))
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
            session.ui.table_state.select(Some(0));
            SessionAction::None
        }
        Cmd::LastRow => {
            let count = session.row_count();
            if count > 0 {
                session.ui.table_state.select(Some(count - 1));
            }
            SessionAction::None
        }
        Cmd::StartFilter => {
            session.ui.mode = SessionMode::Filter;
            session.ui.filter_input.clear();
            SessionAction::None
        }
        Cmd::ToggleHideSame => {
            if let Some(dir) = session.dir_mut() {
                dir.hide_same = !dir.hide_same;
            }
            session.ui.table_state.select(Some(0));
            SessionAction::None
        }
        Cmd::ToggleCompareContent => {
            if let Some(dir) = session.dir_mut() {
                dir.compare_content = !dir.compare_content;
            }
            SessionAction::None
        }
        Cmd::Sync => {
            // Plan the sync with the current operation (dry-run); the
            // controller opens the confirmation modal once the plan
            // arrives. Bound in the dir scope only, so the dir state is
            // present.
            SessionAction::PlanSync {
                operation: session
                    .dir()
                    .map(|d| d.sync_operation)
                    .unwrap_or_default(),
            }
        }
        Cmd::CycleSyncOpForward => {
            if let Some(dir) = session.dir_mut() {
                dir.sync_operation = cycle_sync_op(&dir.sync_operation, 1);
            }
            SessionAction::None
        }
        Cmd::CycleSyncOpBackward => {
            if let Some(dir) = session.dir_mut() {
                dir.sync_operation = cycle_sync_op(&dir.sync_operation, -1);
            }
            SessionAction::None
        }
        Cmd::StartSaveSession => {
            // Enter input mode; the name buffer lives in the mode itself.
            session.ui.mode = SessionMode::SaveSession {
                name: String::new(),
            };
            SessionAction::None
        }
        Cmd::StartLoadSession => {
            // Enter load mode and list saved sessions for it.
            session.ui.mode = SessionMode::LoadSession {
                name: String::new(),
                saved_sessions: Vec::new(),
            };
            SessionAction::ListSessions
        }
        Cmd::Reload => SessionAction::Reload,
        Cmd::OpenEntry => handle_enter(session),
        Cmd::GoUp => {
            // Inside an expanded subtree, Backspace collapses the
            // containing directory and lands on its row; at the top level
            // it still asks the controller to open the parent comparison.
            let parent = session.selected_row().and_then(|row| {
                row.rel_path
                    .rsplit_once('/')
                    .map(|(parent, _)| parent.to_string())
            });
            match parent {
                Some(parent) => {
                    collapse_dir(session, &parent);
                    SessionAction::None
                }
                None => SessionAction::GoUp,
            }
        }
        Cmd::Cancel => {
            // If a sync preview is active, cancel it. Otherwise, clear the
            // filter.
            let cancelled_sync = session
                .dir_mut()
                .is_some_and(|dir| dir.sync.take().is_some());
            if !cancelled_sync {
                session.ui.active_filter = None;
            }
            session.ui.table_state.select(Some(0));
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
        Cmd::ExpandDir => {
            toggle_selected_dir(session, true);
            SessionAction::None
        }
        Cmd::CollapseDir => {
            toggle_selected_dir(session, false);
            SessionAction::None
        }
        Cmd::CopyDialog => open_dialog(session, TransferOp::Copy),
        Cmd::DeleteDialog => open_dialog(session, TransferOp::Delete),
        Cmd::DialogPick(choice) => dialog_choice(session, choice),
        Cmd::DialogAbort => {
            session.ui.mode = SessionMode::Normal;
            SessionAction::None
        }
        Cmd::StartSearch => {
            // Enter search mode; the query buffer lives in the mode itself.
            session.ui.mode = SessionMode::Search {
                query: String::new(),
            };
            SessionAction::None
        }
        Cmd::StartCommand => {
            // Enter command mode; the input buffer lives in the mode.
            session.ui.mode = SessionMode::Command {
                input: String::new(),
            };
            SessionAction::None
        }
        Cmd::CancelEdit => {
            session.ui.mode = SessionMode::Normal;
            session.ui.filter_input.clear();
            SessionAction::None
        }
        // Global commands are dispatched by the App shell, never here.
        Cmd::CloseTab
        | Cmd::SwitchTab
        | Cmd::MenuPrev
        | Cmd::MenuNext
        | Cmd::Quit
        | Cmd::ToggleHelp => SessionAction::None,
    }
}

/// Move the selection by `delta` rows, clamped to the session's row count.
fn navigate(session: &mut SessionView, delta: isize) {
    let rows = session.row_count();
    if delta > 0 {
        if let Some(selected) = session.ui.table_state.selected() {
            if selected + 1 < rows {
                session.ui.table_state.select(Some(selected + 1));
            }
        } else if rows > 0 {
            session.ui.table_state.select(Some(0));
        }
    } else if let Some(selected) =
        session.ui.table_state.selected().filter(|s| *s > 0)
    {
        session.ui.table_state.select(Some(selected - 1));
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

    let current = session.ui.table_state.selected().unwrap_or(0);
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
    session
        .ui
        .table_state
        .select(Some(target.unwrap_or(fallback)));
}

/// Handle Enter key — open the selected entry in a new session, leaving
/// the tree of the current session in place.
fn handle_enter(session: &SessionView) -> SessionAction {
    let Some(row) = session.selected_row() else {
        return SessionAction::None;
    };
    let selected = row.entry;

    // Build the sub-endpoints for the new comparison. An entry's `path` is
    // relative to the current comparison root, so it must be joined onto the
    // endpoint's path rather than used as the endpoint path directly (which
    // would drop the root and resolve to a non-existent location). An entry
    // that only exists on one side falls back to the row's root-relative
    // path on the other side, which then reads as a non-existent file or
    // directory.
    let left_sub = selected
        .left
        .as_ref()
        .map(|l| session.core.left.join(&l.path))
        .or_else(|| Some(session.core.left.join(&row.rel_path)));
    let right_sub = selected
        .right
        .as_ref()
        .map(|r| session.core.right.join(&r.path))
        .or_else(|| Some(session.core.right.join(&row.rel_path)));

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

/// Expand (`open`) or collapse the directory on the selected row.
///
/// A no-op on a file row, on a row that is already in the requested
/// state, or when nothing is selected.
fn toggle_selected_dir(session: &mut SessionView, open: bool) {
    let Some((rel_path, is_dir)) =
        session.selected_row().map(|row| (row.rel_path, row.is_dir))
    else {
        return;
    };
    if !is_dir {
        return;
    }
    let Some(dir) = session.dir_mut() else {
        return;
    };
    let changed = if open {
        dir.expanded.insert(rel_path)
    } else {
        dir.expanded.remove(&rel_path)
    };
    if changed {
        dir.tree_revision += 1;
    }
}

/// Collapse the expanded directory at `path` and move the selection onto
/// its row, so stepping out of a subtree lands on its parent.
fn collapse_dir(session: &mut SessionView, path: &str) {
    if let Some(dir) = session.dir_mut()
        && dir.expanded.remove(path)
    {
        dir.tree_revision += 1;
    }
    if let Some(index) = session.row_index_of(path) {
        session.ui.table_state.select(Some(index));
    }
}

/// Cycle through sync operations.
fn cycle_sync_op(current: &SyncOperation, delta: isize) -> SyncOperation {
    let ops = SyncOperation::ALL;
    let len = ops.len() as isize;
    let idx = ops.iter().position(|x| x == current).unwrap_or(0) as isize;
    let new_idx = ((idx + delta) % len + len) % len;
    ops[new_idx as usize]
}

/// Open the transfer dialog for the selected entry.
///
/// A no-op when nothing is selected (or the selected entry exists on
/// neither side), so `c`/`d` on an empty table simply do nothing.
fn open_dialog(session: &mut SessionView, op: TransferOp) -> SessionAction {
    if session.transfer_target().is_some() {
        session.ui.mode = SessionMode::Transfer { op };
    }
    SessionAction::None
}

/// Resolve a transfer-dialog option into the action the controller must
/// dispatch.
///
/// The target is resolved here rather than trusted from the dialog's render,
/// so an accelerator for an option the entry does not activate is absorbed:
/// the dialog stays open and nothing transfers. Only an active option (or
/// `abort`) closes the dialog.
pub fn dialog_choice(
    session: &mut SessionView,
    choice: TransferChoice,
) -> SessionAction {
    let SessionMode::Transfer { op } = session.ui.mode else {
        return SessionAction::None;
    };
    let Some(target) = session.transfer_target() else {
        // The dialog cannot render without a target; drop the stale mode.
        session.ui.mode = SessionMode::Normal;
        return SessionAction::None;
    };
    if !choice.is_enabled(op, target.has_left, target.has_right) {
        return SessionAction::None;
    }
    session.ui.mode = SessionMode::Normal;
    let items = target.items(op, choice);
    if items.is_empty() {
        SessionAction::None
    } else {
        SessionAction::Transfer { items }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        path::{Path, PathBuf},
        sync::Arc,
    };

    use cocomo_lib::{
        DirComparison, DirEntry, DirEntryStatus, Endpoint, EntryInfo, Grammar,
        LineInfo, TextDiff, TextDifference, TransferAction,
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
            Endpoint::local(PathBuf::from("/left")),
            Endpoint::local(PathBuf::from("/right")),
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
            Endpoint::local(PathBuf::from("/left")),
            Endpoint::local(PathBuf::from("/right")),
            false,
        );
        session.set_comparison(Some(DirComparison {
            entries,
            ..Default::default()
        }));
        session
    }

    fn press(
        session: &mut SessionView,
        code: KeyCode,
    ) -> Option<SessionAction> {
        handle_session_key(session, key(code))
    }

    /// Dir session with one left-only, one right-only, and one differing
    /// entry, each present on exactly the sides its status implies.
    fn dir_session_with_sides() -> SessionView {
        let info = |name: &str| EntryInfo {
            name: name.into(),
            path: name.into(),
            is_dir: false,
            ..Default::default()
        };
        let entry = |name: &str, status, left: bool, right: bool| DirEntry {
            name: name.into(),
            status,
            left: left.then(|| info(name)),
            right: right.then(|| info(name)),
            center: None,
            sub_entries: None,
        };
        let mut session = SessionView::new_dir_compare(
            Endpoint::local(std::path::PathBuf::from("/left")),
            Endpoint::local(std::path::PathBuf::from("/right")),
            false,
        );
        session.set_comparison(Some(DirComparison {
            entries: vec![
                entry("left-only.txt", DirEntryStatus::LeftOnly, true, false),
                entry(
                    "right-only.txt",
                    DirEntryStatus::RightOnly,
                    false,
                    true,
                ),
                entry("both.txt", DirEntryStatus::Different, true, true),
            ],
            ..Default::default()
        }));
        session
    }

    /// Dir session with one expandable directory (`sub/` with two files)
    /// and one top-level file.
    fn dir_session_with_tree() -> SessionView {
        let file = |name: &str, path: &str| DirEntry {
            name: name.into(),
            status: DirEntryStatus::Different,
            left: Some(EntryInfo {
                name: name.into(),
                path: path.into(),
                ..Default::default()
            }),
            right: Some(EntryInfo {
                name: name.into(),
                path: path.into(),
                ..Default::default()
            }),
            center: None,
            sub_entries: None,
        };
        let mut sub = file("sub", "sub");
        sub.left.as_mut().unwrap().is_dir = true;
        sub.right.as_mut().unwrap().is_dir = true;
        // Sub-entries carry bare names and root-relative paths, exactly as
        // the scanner produces them.
        sub.sub_entries = Some(Arc::new(DirComparison {
            entries: vec![
                file("a.txt", "sub/a.txt"),
                file("b.txt", "sub/b.txt"),
            ],
            ..Default::default()
        }));
        let mut session = SessionView::new_dir_compare(
            Endpoint::local(PathBuf::from("/left")),
            Endpoint::local(PathBuf::from("/right")),
            false,
        );
        session.set_comparison(Some(DirComparison {
            entries: vec![sub, file("top.txt", "top.txt")],
            ..Default::default()
        }));
        session
    }

    #[test]
    fn row_count_matches_session_type() {
        assert_eq!(dir_session().row_count(), 3);
        assert_eq!(text_session().row_count(), 10);
    }

    #[test]
    fn enter_joins_relative_entry_path_onto_root() {
        // Regression: an entry's `path` is relative to the comparison root,
        // so opening it must join onto the endpoint path, not replace it
        // (replacing dropped the root and produced a "not found" error).
        let mut session = SessionView::new_dir_compare(
            Endpoint::local(PathBuf::from("/left")),
            Endpoint::local(PathBuf::from("/right")),
            false,
        );
        session.set_comparison(Some(DirComparison {
            entries: vec![
                DirEntry {
                    name: "sub".into(),
                    status: DirEntryStatus::Different,
                    left: Some(EntryInfo {
                        name: "sub".into(),
                        path: "sub".into(),
                        is_dir: true,
                        ..Default::default()
                    }),
                    right: Some(EntryInfo {
                        name: "sub".into(),
                        path: "sub".into(),
                        is_dir: true,
                        ..Default::default()
                    }),
                    center: None,
                    sub_entries: None,
                },
                DirEntry {
                    name: "orphan.txt".into(),
                    status: DirEntryStatus::LeftOnly,
                    left: Some(EntryInfo {
                        name: "orphan.txt".into(),
                        path: "orphan.txt".into(),
                        is_dir: false,
                        ..Default::default()
                    }),
                    right: None,
                    center: None,
                    sub_entries: None,
                },
            ],
            ..Default::default()
        }));

        // A directory present on both sides opens a dir comparison rooted at
        // `<root>/sub` on each side.
        session.ui.table_state.select(Some(0));
        match press(&mut session, KeyCode::Enter) {
            Some(SessionAction::OpenDir { left, right }) => {
                assert_eq!(left.path(), Path::new("/left/sub"));
                assert_eq!(right.path(), Path::new("/right/sub"));
            }
            other => panic!("expected OpenDir, got {other:?}"),
        }

        // A file present only on the left opens a text diff; the missing
        // right side falls back to `<root>/orphan.txt`.
        session.ui.table_state.select(Some(1));
        match press(&mut session, KeyCode::Enter) {
            Some(SessionAction::OpenFile { left, right }) => {
                assert_eq!(left.path(), Path::new("/left/orphan.txt"));
                assert_eq!(right.path(), Path::new("/right/orphan.txt"));
            }
            other => panic!("expected OpenFile, got {other:?}"),
        }
    }

    #[test]
    fn j_k_navigate_text_rows() {
        // Regression: j/k used to derive the row count from
        // `filtered_entries()`, which is empty for text sessions.
        let mut session = text_session();
        assert!(press(&mut session, KeyCode::Char('j')).is_some());
        assert_eq!(session.ui.table_state.selected(), Some(0));
        press(&mut session, KeyCode::Char('j'));
        assert_eq!(session.ui.table_state.selected(), Some(1));
        press(&mut session, KeyCode::Char('k'));
        assert_eq!(session.ui.table_state.selected(), Some(0));
        // Clamped at the top.
        press(&mut session, KeyCode::Char('k'));
        assert_eq!(session.ui.table_state.selected(), Some(0));
    }

    #[test]
    fn shift_g_jumps_to_bounds() {
        let mut session = text_session();
        press(&mut session, KeyCode::Char('G'));
        assert_eq!(session.ui.table_state.selected(), Some(9));
        press(&mut session, KeyCode::Char('j'));
        // Clamped at the bottom.
        assert_eq!(session.ui.table_state.selected(), Some(9));
        press(&mut session, KeyCode::Char('g'));
        assert_eq!(session.ui.table_state.selected(), Some(0));
    }

    #[test]
    fn n_jumps_to_next_diff_and_wraps() {
        let mut session = text_session();
        session.ui.table_state.select(Some(0));
        press(&mut session, KeyCode::Char('n'));
        assert_eq!(session.ui.table_state.selected(), Some(2));
        press(&mut session, KeyCode::Char('n'));
        assert_eq!(session.ui.table_state.selected(), Some(6));
        // Wrap to the first group.
        press(&mut session, KeyCode::Char('n'));
        assert_eq!(session.ui.table_state.selected(), Some(2));
    }

    #[test]
    fn shift_n_jumps_to_prev_diff_and_wraps() {
        // Regression: N used to search for groups *after* the cursor, so it
        // jumped forward instead of backward.
        let mut session = text_session();
        session.ui.table_state.select(Some(9));
        press(&mut session, KeyCode::Char('N'));
        assert_eq!(session.ui.table_state.selected(), Some(6));
        press(&mut session, KeyCode::Char('N'));
        assert_eq!(session.ui.table_state.selected(), Some(2));
        // Wrap to the last group.
        press(&mut session, KeyCode::Char('N'));
        assert_eq!(session.ui.table_state.selected(), Some(6));
    }

    #[test]
    fn unbound_keys_are_not_consumed() {
        let mut session = dir_session();
        // `x` is bound nowhere.
        assert_eq!(press(&mut session, KeyCode::Char('x')), None);
        // `q` is owned by the App shell, not the session handler.
        assert_eq!(press(&mut session, KeyCode::Char('q')), None);
        // The two-step sync keys `P`/`e` are gone; `S` is the only sync
        // entry point.
        assert_eq!(press(&mut session, KeyCode::Char('P')), None);
        assert_eq!(press(&mut session, KeyCode::Char('e')), None);
        // Dir-only keys are unbound in a text session.
        let mut session = text_session();
        assert_eq!(press(&mut session, KeyCode::Char('f')), None);
    }

    #[test]
    fn editing_mode_esc_cancels_and_other_keys_pass_through() {
        let mut session = dir_session();
        session.ui.mode = SessionMode::Filter;
        session.ui.filter_input = "partial".into();
        let action = press(&mut session, KeyCode::Esc);
        assert_eq!(action, Some(SessionAction::None));
        assert_eq!(session.ui.mode, SessionMode::Normal);
        assert!(session.ui.filter_input.is_empty());

        // While editing, printable keys are not owned by this handler.
        session.ui.mode = SessionMode::Filter;
        assert_eq!(press(&mut session, KeyCode::Char('x')), None);
        // `q` cancels the mode instead of quitting.
        assert!(press(&mut session, KeyCode::Char('q')).is_some());
        assert_eq!(session.ui.mode, SessionMode::Normal);
    }

    #[test]
    fn save_and_load_modes_carry_their_own_buffers() {
        let mut session = dir_session();
        session.ui.filter_input = "keep me".into();

        // Save mode owns its name buffer; the filter buffer is untouched.
        assert_eq!(
            press(&mut session, KeyCode::Char('s')),
            Some(SessionAction::None)
        );
        assert!(matches!(
            &session.ui.mode,
            SessionMode::SaveSession { name } if name.is_empty()
        ));
        assert_eq!(session.ui.filter_input, "keep me");

        // Cancelling drops the mode and its buffer with it.
        assert!(press(&mut session, KeyCode::Char('q')).is_some());
        assert_eq!(session.ui.mode, SessionMode::Normal);

        // Load mode carries its own name buffer and saved-session list,
        // and asks the controller to fill the list.
        assert_eq!(
            press(&mut session, KeyCode::Char('O')),
            Some(SessionAction::ListSessions)
        );
        assert!(matches!(
            &session.ui.mode,
            SessionMode::LoadSession { name, saved_sessions }
                if name.is_empty() && saved_sessions.is_empty()
        ));
    }

    #[test]
    fn tab_switches_tabs_only_in_the_shell() {
        // Bare Tab and Ctrl+Tab belong to the shell; neither fires in the
        // session scopes, and a NONE-modifier binding never fires for a
        // Ctrl combination.
        assert_eq!(
            lookup(KeyScope::Global, key(KeyCode::Tab)),
            Some(Cmd::SwitchTab)
        );
        let ctrl_tab = KeyEvent::new(KeyCode::Tab, CTRL | KeyModifiers::SHIFT);
        assert_eq!(lookup(KeyScope::Global, ctrl_tab), Some(Cmd::SwitchTab));
        assert_eq!(lookup(KeyScope::DirNormal, ctrl_tab), None);
        assert_eq!(lookup(KeyScope::DirNormal, key(KeyCode::Tab)), None);
        assert_eq!(lookup(KeyScope::TextNormal, key(KeyCode::Tab)), None);
    }

    #[test]
    fn plus_expands_and_minus_collapses_the_selected_dir() {
        let mut session = dir_session_with_tree();
        assert_eq!(session.row_count(), 2);
        session.ui.table_state.select(Some(0));
        assert_eq!(
            press(&mut session, KeyCode::Char('+')),
            Some(SessionAction::None)
        );
        assert_eq!(session.row_count(), 4);
        assert!(session.dir().unwrap().expanded.contains("sub"));
        // The children are visible and reachable by navigation.
        let rows = session.visible_rows();
        assert_eq!(rows[1].rel_path, "sub/a.txt");
        assert_eq!(rows[1].depth, 1);

        assert_eq!(
            press(&mut session, KeyCode::Char('-')),
            Some(SessionAction::None)
        );
        assert_eq!(session.row_count(), 2);
        assert!(session.dir().unwrap().expanded.is_empty());
        // The selection stays on the directory row throughout.
        assert_eq!(session.ui.table_state.selected(), Some(0));
    }

    #[test]
    fn plus_on_a_file_row_is_a_noop() {
        let mut session = dir_session_with_tree();
        session.ui.table_state.select(Some(1));
        press(&mut session, KeyCode::Char('+'));
        assert!(session.dir().unwrap().expanded.is_empty());
        assert_eq!(session.row_count(), 2);
    }

    #[test]
    fn backspace_collapses_the_containing_directory() {
        let mut session = dir_session_with_tree();
        session.ui.table_state.select(Some(0));
        press(&mut session, KeyCode::Char('+'));
        // Select the second child (`sub/b.txt`, row 2).
        session.ui.table_state.select(Some(2));
        assert_eq!(
            press(&mut session, KeyCode::Backspace),
            Some(SessionAction::None)
        );
        // The subtree collapsed and the selection landed on the parent.
        assert_eq!(session.row_count(), 2);
        assert_eq!(session.ui.table_state.selected(), Some(0));
        assert!(session.dir().unwrap().expanded.is_empty());
    }

    #[test]
    fn backspace_at_the_root_still_steps_out() {
        let mut session = dir_session_with_tree();
        session.ui.table_state.select(Some(1));
        assert_eq!(
            press(&mut session, KeyCode::Backspace),
            Some(SessionAction::GoUp)
        );
    }

    #[test]
    fn enter_on_a_child_joins_its_root_relative_path() {
        // A nested entry's `path` is relative to the comparison root, so
        // opening it in a new tab must produce `<root>/sub/a.txt`.
        let mut session = dir_session_with_tree();
        session.ui.table_state.select(Some(0));
        press(&mut session, KeyCode::Char('+'));
        session.ui.table_state.select(Some(1));
        match press(&mut session, KeyCode::Enter) {
            Some(SessionAction::OpenFile { left, right }) => {
                assert_eq!(left.path(), Path::new("/left/sub/a.txt"));
                assert_eq!(right.path(), Path::new("/right/sub/a.txt"));
            }
            other => panic!("expected OpenFile, got {other:?}"),
        }
    }

    #[test]
    fn n_navigates_dir_differences() {
        // In the dir view, every non-identical entry is a difference group;
        // identical entries are skipped and the jump wraps.
        let mut session = dir_session();
        session.set_comparison(Some(DirComparison {
            entries: vec![
                DirEntry {
                    name: "a".into(),
                    status: DirEntryStatus::Same,
                    left: None,
                    right: None,
                    center: None,
                    sub_entries: None,
                },
                DirEntry {
                    name: "b".into(),
                    status: DirEntryStatus::Different,
                    left: None,
                    right: None,
                    center: None,
                    sub_entries: None,
                },
                DirEntry {
                    name: "c".into(),
                    status: DirEntryStatus::SameBinary,
                    left: None,
                    right: None,
                    center: None,
                    sub_entries: None,
                },
                DirEntry {
                    name: "d".into(),
                    status: DirEntryStatus::LeftOnly,
                    left: None,
                    right: None,
                    center: None,
                    sub_entries: None,
                },
            ],
            ..Default::default()
        }));
        session.ui.table_state.select(Some(0));
        press(&mut session, KeyCode::Char('n'));
        assert_eq!(session.ui.table_state.selected(), Some(1));
        press(&mut session, KeyCode::Char('n'));
        assert_eq!(session.ui.table_state.selected(), Some(3));
        // Wrap to the first difference.
        press(&mut session, KeyCode::Char('n'));
        assert_eq!(session.ui.table_state.selected(), Some(1));
        press(&mut session, KeyCode::Char('N'));
        assert_eq!(session.ui.table_state.selected(), Some(3));
    }

    #[test]
    fn c_and_d_open_the_transfer_dialog() {
        for (key, op) in [('c', TransferOp::Copy), ('d', TransferOp::Delete)] {
            let mut session = dir_session_with_sides();
            session.ui.table_state.select(Some(0));
            assert_eq!(
                press(&mut session, KeyCode::Char(key)),
                Some(SessionAction::None)
            );
            assert_eq!(session.ui.mode, SessionMode::Transfer { op });
        }
    }

    #[test]
    fn dialog_keys_work_in_a_text_session_too() {
        let mut session = text_session();
        assert_eq!(
            press(&mut session, KeyCode::Char('d')),
            Some(SessionAction::None)
        );
        assert_eq!(
            session.ui.mode,
            SessionMode::Transfer {
                op: TransferOp::Delete
            }
        );
    }

    #[test]
    fn dialog_needs_a_target_entry() {
        // The entries of `dir_session()` exist on neither side, so there is
        // nothing to copy or delete and the dialog stays closed.
        let mut session = dir_session();
        session.ui.table_state.select(Some(0));
        press(&mut session, KeyCode::Char('c'));
        assert_eq!(session.ui.mode, SessionMode::Normal);
    }

    #[test]
    fn dialog_accelerator_builds_the_transfer_items() {
        let mut session = dir_session_with_sides();
        session.ui.table_state.select(Some(0));
        press(&mut session, KeyCode::Char('c'));
        // `l` is the left → right accelerator of the copy dialog.
        match press(&mut session, KeyCode::Char('l')) {
            Some(SessionAction::Transfer { items }) => {
                assert_eq!(items.len(), 1);
                assert_eq!(items[0].action, TransferAction::CopyRight);
                assert_eq!(items[0].name, "left-only.txt");
                assert_eq!(items[0].rel_path, "left-only.txt");
                assert!(!items[0].is_dir);
            }
            other => panic!("expected Transfer, got {other:?}"),
        }
        // Confirming closes the dialog.
        assert_eq!(session.ui.mode, SessionMode::Normal);
    }

    #[test]
    fn delete_both_transfers_one_item_per_side() {
        let mut session = dir_session_with_sides();
        session.ui.table_state.select(Some(2));
        press(&mut session, KeyCode::Char('d'));
        match press(&mut session, KeyCode::Char('b')) {
            Some(SessionAction::Transfer { items }) => {
                assert_eq!(items.len(), 2);
                assert_eq!(items[0].action, TransferAction::DeleteLeft);
                assert_eq!(items[1].action, TransferAction::DeleteRight);
            }
            other => panic!("expected Transfer, got {other:?}"),
        }
    }

    #[test]
    fn dead_option_absorbs_its_accelerator() {
        // A right-only entry does not activate `left → right`, so `l` is a
        // no-op that leaves the dialog open; the active `r` still transfers.
        let mut session = dir_session_with_sides();
        session.ui.table_state.select(Some(1));
        press(&mut session, KeyCode::Char('c'));
        assert_eq!(
            press(&mut session, KeyCode::Char('l')),
            Some(SessionAction::None)
        );
        assert_eq!(
            session.ui.mode,
            SessionMode::Transfer {
                op: TransferOp::Copy
            }
        );
        match press(&mut session, KeyCode::Char('r')) {
            Some(SessionAction::Transfer { items }) => {
                assert_eq!(items[0].action, TransferAction::CopyLeft);
            }
            other => panic!("expected Transfer, got {other:?}"),
        }
        assert_eq!(session.ui.mode, SessionMode::Normal);
    }

    #[test]
    fn dialog_abort_leaves_the_data_alone() {
        let mut session = dir_session_with_sides();
        session.ui.table_state.select(Some(0));
        press(&mut session, KeyCode::Char('d'));
        assert_eq!(
            press(&mut session, KeyCode::Esc),
            Some(SessionAction::None)
        );
        assert_eq!(session.ui.mode, SessionMode::Normal);

        // `a` aborts just like Esc.
        press(&mut session, KeyCode::Char('d'));
        assert_eq!(
            press(&mut session, KeyCode::Char('a')),
            Some(SessionAction::None)
        );
        assert_eq!(session.ui.mode, SessionMode::Normal);
    }

    #[test]
    fn dialog_scope_owns_only_its_keys() {
        // While the dialog is open the session handler claims nothing but
        // `l`/`r`/`b`/`Esc`/`a`; `j` and the dir-scope commands stay
        // unbound (the dialog's exclusive input layer swallows them, so the
        // table behind cannot move).
        let mut session = dir_session_with_sides();
        session.ui.table_state.select(Some(0));
        press(&mut session, KeyCode::Char('d'));
        assert_eq!(press(&mut session, KeyCode::Char('j')), None);
        assert_eq!(session.ui.table_state.selected(), Some(0));
        // Dir-scope commands are unreachable until the dialog closes; `n`
        // is bound in the dir scope but not a dialog key.
        assert_eq!(press(&mut session, KeyCode::Char('n')), None);
        assert_eq!(
            session.ui.mode,
            SessionMode::Transfer {
                op: TransferOp::Delete
            }
        );
    }

    #[test]
    fn s_plans_sync_for_confirmation() {
        // `S` requests the dry-run plan; the controller opens the
        // confirmation modal once the plan arrives.
        let mut session = dir_session();
        assert_eq!(
            press(&mut session, KeyCode::Char('S')),
            Some(SessionAction::PlanSync {
                operation: SyncOperation::default()
            })
        );
    }

    #[test]
    fn sync_confirm_mode_claims_no_session_keys() {
        // While the confirmation modal is open its exclusive input layer
        // owns every key, so the session handler must claim nothing: even
        // bound keys like `j` and `Esc` report as unbound (the layer
        // swallows them) and the selection cannot move behind the modal.
        let mut session = dir_session();
        session.ui.mode = SessionMode::SyncConfirm;
        assert_eq!(press(&mut session, KeyCode::Char('j')), None);
        assert_eq!(session.ui.table_state.selected(), Some(0));
        assert_eq!(press(&mut session, KeyCode::Esc), None);
    }

    #[test]
    fn slash_and_colon_open_editing_modes() {
        let mut session = dir_session();
        assert_eq!(
            press(&mut session, KeyCode::Char('/')),
            Some(SessionAction::None)
        );
        assert!(matches!(
            &session.ui.mode,
            SessionMode::Search { query } if query.is_empty()
        ));
        assert!(session.ui.mode.is_editing());
        // Esc (Editing scope) cancels the mode.
        press(&mut session, KeyCode::Esc);
        assert_eq!(session.ui.mode, SessionMode::Normal);

        assert_eq!(
            press(&mut session, KeyCode::Char(':')),
            Some(SessionAction::None)
        );
        assert!(matches!(
            &session.ui.mode,
            SessionMode::Command { input } if input.is_empty()
        ));
        assert!(session.ui.mode.is_editing());
    }

    #[test]
    fn help_lines_contain_all_labelled_bindings_of_their_scopes() {
        let lines =
            help_lines(&[KeyScope::TextNormal, KeyScope::Global], u16::MAX);
        let text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        for label in ["j/k", "g/G", "n/N", "Ctrl+W", "Tab", "q"] {
            assert!(text.contains(label), "missing `{label}` in `{text}`");
        }
        // Dir-only labels must not appear in the text help bar.
        assert!(!text.contains("filter"));
    }

    #[test]
    fn help_lines_wrap_to_the_given_width() {
        let width: u16 = 40;
        let scopes = [KeyScope::TextNormal, KeyScope::Global];
        let wide = help_lines(&scopes, u16::MAX);
        let narrow = help_lines(&scopes, width);
        // Premise: the unwrapped bar is wider than the test width.
        assert!(
            wide.iter().map(|l| l.width()).sum::<usize>() > usize::from(width),
            "keymap too small to exercise wrapping",
        );
        assert!(narrow.len() > 1, "narrow width must wrap to several lines");
        for line in &narrow {
            assert!(
                line.width() <= usize::from(width),
                "line wider than `{width}`: `{line}`",
            );
        }
        // Wrapping must not drop items: the same bold key labels appear.
        let bold_labels = |lines: &[Line<'static>]| {
            lines
                .iter()
                .flat_map(|l| l.spans.iter())
                .filter(|s| s.style.add_modifier.contains(Modifier::BOLD))
                .count()
        };
        assert_eq!(bold_labels(&narrow), bold_labels(&wide));
    }

    #[test]
    fn help_toggle_is_bound_in_the_shell() {
        // `?` (typed with Shift on US layouts) and F1 both toggle the help
        // bar; a NONE-modifier binding still fires for the Shift combo.
        let question = KeyEvent::new(KeyCode::Char('?'), KeyModifiers::SHIFT);
        assert_eq!(lookup(KeyScope::Global, question), Some(Cmd::ToggleHelp));
        assert_eq!(
            lookup(KeyScope::Global, key(KeyCode::F(1))),
            Some(Cmd::ToggleHelp)
        );
        assert_eq!(lookup(KeyScope::DirNormal, key(KeyCode::F(1))), None);
    }
}
