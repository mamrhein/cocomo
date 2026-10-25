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
//! Bindings may also declare a prefix key, forming two-key sequences such
//! as `D l` (delete left); the pending prefix lives in
//! [`crate::state::UiState::pending_prefix`].
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

use cocomo_lib::{SyncOperation, TransferAction, TransferItem};
use ratatui_kit::{
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
    ratatui::{
        style::{Modifier, Style},
        text::{Line, Span},
    },
};

use crate::{
    diff_model::diff_group_starts,
    state::{SessionAction, SessionContent, SessionMode, SessionView},
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
    /// Copy the selected entry left → right (`C-l`).
    CopyRight,
    /// Copy the selected entry right → left (`C-r`).
    CopyLeft,
    /// Delete the selected entry from the left side (`D l`).
    DeleteLeft,
    /// Delete the selected entry from the right side (`D r`).
    DeleteRight,
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
    /// (e.g. the `Down` alias of `j`).
    help: Option<(&'static str, &'static str)>,
    /// Prefix key that must be pressed immediately before `key` for a
    /// two-key sequence binding (e.g. the `D` of `D l`). `None` marks a
    /// single-key binding.
    prefix: Option<KeyCode>,
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
        prefix: None,
    },
    KeyBinding {
        key: KeyCode::Down,
        modifiers: NONE,
        scope: KeyScope::DirNormal,
        cmd: Cmd::NextRow,
        help: None,
        prefix: None,
    },
    KeyBinding {
        key: KeyCode::Char('k'),
        modifiers: NONE,
        scope: KeyScope::DirNormal,
        cmd: Cmd::PrevRow,
        help: None,
        prefix: None,
    },
    KeyBinding {
        key: KeyCode::Up,
        modifiers: NONE,
        scope: KeyScope::DirNormal,
        cmd: Cmd::PrevRow,
        help: None,
        prefix: None,
    },
    KeyBinding {
        key: KeyCode::Char('g'),
        modifiers: NONE,
        scope: KeyScope::DirNormal,
        cmd: Cmd::FirstRow,
        help: Some(("g/G", "top/bottom")),
        prefix: None,
    },
    KeyBinding {
        key: KeyCode::Char('G'),
        modifiers: NONE,
        scope: KeyScope::DirNormal,
        cmd: Cmd::LastRow,
        help: None,
        prefix: None,
    },
    KeyBinding {
        key: KeyCode::Char('n'),
        modifiers: NONE,
        scope: KeyScope::DirNormal,
        cmd: Cmd::NextDiff,
        help: Some(("n/N", "next/prev diff")),
        prefix: None,
    },
    KeyBinding {
        key: KeyCode::Char('N'),
        modifiers: NONE,
        scope: KeyScope::DirNormal,
        cmd: Cmd::PrevDiff,
        help: None,
        prefix: None,
    },
    KeyBinding {
        key: KeyCode::Char('f'),
        modifiers: NONE,
        scope: KeyScope::DirNormal,
        cmd: Cmd::StartFilter,
        help: Some(("f", "filter")),
        prefix: None,
    },
    KeyBinding {
        key: KeyCode::Char('F'),
        modifiers: NONE,
        scope: KeyScope::DirNormal,
        cmd: Cmd::ToggleHideSame,
        help: Some(("F", "hide same")),
        prefix: None,
    },
    KeyBinding {
        key: KeyCode::Char('C'),
        modifiers: NONE,
        scope: KeyScope::DirNormal,
        cmd: Cmd::ToggleCompareContent,
        help: Some(("C", "cont. on/off")),
        prefix: None,
    },
    KeyBinding {
        key: KeyCode::Char('m'),
        modifiers: NONE,
        scope: KeyScope::DirNormal,
        cmd: Cmd::CycleSyncOpForward,
        help: Some(("m/M", "sync op")),
        prefix: None,
    },
    KeyBinding {
        key: KeyCode::Char('M'),
        modifiers: NONE,
        scope: KeyScope::DirNormal,
        cmd: Cmd::CycleSyncOpBackward,
        help: None,
        prefix: None,
    },
    KeyBinding {
        // `S` runs the sync; until the confirmation modal lands (phase 3),
        // it plans the sync exactly like `P`.
        key: KeyCode::Char('S'),
        modifiers: NONE,
        scope: KeyScope::DirNormal,
        cmd: Cmd::PlanSync,
        help: Some(("S", "sync")),
        prefix: None,
    },
    KeyBinding {
        key: KeyCode::Char('P'),
        modifiers: NONE,
        scope: KeyScope::DirNormal,
        cmd: Cmd::PlanSync,
        help: Some(("P", "plan")),
        prefix: None,
    },
    KeyBinding {
        key: KeyCode::Char('e'),
        modifiers: NONE,
        scope: KeyScope::DirNormal,
        cmd: Cmd::ExecuteSync,
        help: Some(("e", "execute")),
        prefix: None,
    },
    KeyBinding {
        key: KeyCode::Char('l'),
        modifiers: CTRL,
        scope: KeyScope::DirNormal,
        cmd: Cmd::CopyRight,
        help: Some(("C-l", "copy l\u{2192}r")),
        prefix: None,
    },
    KeyBinding {
        key: KeyCode::Char('r'),
        modifiers: CTRL,
        scope: KeyScope::DirNormal,
        cmd: Cmd::CopyLeft,
        help: Some(("C-r", "copy r\u{2192}l")),
        prefix: None,
    },
    KeyBinding {
        key: KeyCode::Char('l'),
        modifiers: NONE,
        scope: KeyScope::DirNormal,
        cmd: Cmd::DeleteLeft,
        help: Some(("D l", "del left")),
        prefix: Some(KeyCode::Char('D')),
    },
    KeyBinding {
        key: KeyCode::Char('r'),
        modifiers: NONE,
        scope: KeyScope::DirNormal,
        cmd: Cmd::DeleteRight,
        help: Some(("D r", "del right")),
        prefix: Some(KeyCode::Char('D')),
    },
    KeyBinding {
        key: KeyCode::Char('r'),
        modifiers: NONE,
        scope: KeyScope::DirNormal,
        cmd: Cmd::Reload,
        help: Some(("r", "reload")),
        prefix: None,
    },
    KeyBinding {
        key: KeyCode::Char('s'),
        modifiers: NONE,
        scope: KeyScope::DirNormal,
        cmd: Cmd::StartSaveSession,
        help: Some(("s", "save")),
        prefix: None,
    },
    KeyBinding {
        key: KeyCode::Char('O'),
        modifiers: NONE,
        scope: KeyScope::DirNormal,
        cmd: Cmd::StartLoadSession,
        help: Some(("O", "load")),
        prefix: None,
    },
    KeyBinding {
        key: KeyCode::Char('/'),
        modifiers: NONE,
        scope: KeyScope::DirNormal,
        cmd: Cmd::StartSearch,
        help: Some(("/", "search")),
        prefix: None,
    },
    KeyBinding {
        key: KeyCode::Char(':'),
        modifiers: NONE,
        scope: KeyScope::DirNormal,
        cmd: Cmd::StartCommand,
        help: Some((":", "command")),
        prefix: None,
    },
    KeyBinding {
        key: KeyCode::Enter,
        modifiers: NONE,
        scope: KeyScope::DirNormal,
        cmd: Cmd::OpenEntry,
        help: Some(("\u{21A5}", "open dir")),
        prefix: None,
    },
    KeyBinding {
        key: KeyCode::Backspace,
        modifiers: NONE,
        scope: KeyScope::DirNormal,
        cmd: Cmd::GoUp,
        help: Some(("\u{232B}", "up")),
        prefix: None,
    },
    KeyBinding {
        key: KeyCode::Esc,
        modifiers: NONE,
        scope: KeyScope::DirNormal,
        cmd: Cmd::Cancel,
        help: Some(("Esc", "cancel")),
        prefix: None,
    },
    // -- Text comparison, normal mode. --
    KeyBinding {
        key: KeyCode::Char('j'),
        modifiers: NONE,
        scope: KeyScope::TextNormal,
        cmd: Cmd::NextRow,
        help: Some(("j/k", "navigate")),
        prefix: None,
    },
    KeyBinding {
        key: KeyCode::Down,
        modifiers: NONE,
        scope: KeyScope::TextNormal,
        cmd: Cmd::NextRow,
        help: None,
        prefix: None,
    },
    KeyBinding {
        key: KeyCode::Char('k'),
        modifiers: NONE,
        scope: KeyScope::TextNormal,
        cmd: Cmd::PrevRow,
        help: None,
        prefix: None,
    },
    KeyBinding {
        key: KeyCode::Up,
        modifiers: NONE,
        scope: KeyScope::TextNormal,
        cmd: Cmd::PrevRow,
        help: None,
        prefix: None,
    },
    KeyBinding {
        key: KeyCode::Char('g'),
        modifiers: NONE,
        scope: KeyScope::TextNormal,
        cmd: Cmd::FirstRow,
        help: Some(("g/G", "top/bottom")),
        prefix: None,
    },
    KeyBinding {
        key: KeyCode::Char('G'),
        modifiers: NONE,
        scope: KeyScope::TextNormal,
        cmd: Cmd::LastRow,
        help: None,
        prefix: None,
    },
    KeyBinding {
        key: KeyCode::Char('n'),
        modifiers: NONE,
        scope: KeyScope::TextNormal,
        cmd: Cmd::NextDiff,
        help: Some(("n/N", "next/prev diff")),
        prefix: None,
    },
    KeyBinding {
        key: KeyCode::Char('N'),
        modifiers: NONE,
        scope: KeyScope::TextNormal,
        cmd: Cmd::PrevDiff,
        help: None,
        prefix: None,
    },
    KeyBinding {
        key: KeyCode::Char('/'),
        modifiers: NONE,
        scope: KeyScope::TextNormal,
        cmd: Cmd::StartSearch,
        help: Some(("/", "search")),
        prefix: None,
    },
    KeyBinding {
        key: KeyCode::Char(':'),
        modifiers: NONE,
        scope: KeyScope::TextNormal,
        cmd: Cmd::StartCommand,
        help: Some((":", "command")),
        prefix: None,
    },
    // -- Text-entry modes (root-layer fallback; the input layers normally
    //    capture keys exclusively). --
    KeyBinding {
        key: KeyCode::Esc,
        modifiers: NONE,
        scope: KeyScope::Editing,
        cmd: Cmd::CancelEdit,
        help: None,
        prefix: None,
    },
    KeyBinding {
        key: KeyCode::Char('q'),
        modifiers: NONE,
        scope: KeyScope::Editing,
        cmd: Cmd::CancelEdit,
        help: None,
        prefix: None,
    },
    // -- App shell. --
    KeyBinding {
        key: KeyCode::Char('w'),
        modifiers: CTRL,
        scope: KeyScope::Global,
        cmd: Cmd::CloseTab,
        help: Some(("Ctrl+W", "close")),
        prefix: None,
    },
    KeyBinding {
        key: KeyCode::Tab,
        modifiers: NONE,
        scope: KeyScope::Global,
        cmd: Cmd::SwitchTab,
        help: Some(("Tab", "tabs")),
        prefix: None,
    },
    KeyBinding {
        key: KeyCode::Tab,
        modifiers: CTRL,
        scope: KeyScope::Global,
        cmd: Cmd::SwitchTab,
        help: None,
        prefix: None,
    },
    KeyBinding {
        key: KeyCode::Left,
        modifiers: NONE,
        scope: KeyScope::Global,
        cmd: Cmd::MenuPrev,
        help: Some(("\u{2190}/\u{2192}", "menu")),
        prefix: None,
    },
    KeyBinding {
        key: KeyCode::Right,
        modifiers: NONE,
        scope: KeyScope::Global,
        cmd: Cmd::MenuNext,
        help: None,
        prefix: None,
    },
    KeyBinding {
        key: KeyCode::Char('q'),
        modifiers: NONE,
        scope: KeyScope::Global,
        cmd: Cmd::Quit,
        help: Some(("q", "quit")),
        prefix: None,
    },
    KeyBinding {
        key: KeyCode::Char('Q'),
        modifiers: NONE,
        scope: KeyScope::Global,
        cmd: Cmd::Quit,
        help: None,
        prefix: None,
    },
    KeyBinding {
        key: KeyCode::Char('?'),
        modifiers: NONE,
        scope: KeyScope::Global,
        cmd: Cmd::ToggleHelp,
        help: Some(("?", "help")),
        prefix: None,
    },
    KeyBinding {
        key: KeyCode::F(1),
        modifiers: NONE,
        scope: KeyScope::Global,
        cmd: Cmd::ToggleHelp,
        help: None,
        prefix: None,
    },
];

/// Return the command bound to `key` in `scope`, if any.
///
/// A binding matches when the key code is equal, the required modifiers are
/// present, and — for unmodified bindings — no Ctrl/Alt modifier is set, so
/// `Ctrl+Tab` never fires the plain `Tab` binding. Two-key sequence
/// bindings (those with a prefix) are excluded; use [`lookup_prefixed`] for
/// their second key.
pub fn lookup(scope: KeyScope, key: KeyEvent) -> Option<Cmd> {
    KEYMAP
        .iter()
        .find(|b| {
            b.prefix.is_none()
                && b.scope == scope
                && b.key == key.code
                && key.modifiers.contains(b.modifiers)
                && (b.modifiers != NONE
                    || !key.modifiers.intersects(CTRL | KeyModifiers::ALT))
        })
        .map(|b| b.cmd)
}

/// Return the command for the second key of a two-key sequence: the binding
/// in `scope` whose prefix is `prefix` and whose key is `key`.
pub fn lookup_prefixed(
    scope: KeyScope,
    prefix: KeyCode,
    key: KeyEvent,
) -> Option<Cmd> {
    KEYMAP
        .iter()
        .find(|b| {
            b.prefix == Some(prefix)
                && b.scope == scope
                && b.key == key.code
                && key.modifiers.contains(b.modifiers)
                && (b.modifiers != NONE
                    || !key.modifiers.intersects(CTRL | KeyModifiers::ALT))
        })
        .map(|b| b.cmd)
}

/// Return whether `key` starts a two-key sequence in `scope` (i.e. some
/// binding uses it as its prefix).
pub fn starts_sequence(scope: KeyScope, key: KeyEvent) -> bool {
    KEYMAP.iter().any(|b| {
        b.prefix == Some(key.code)
            && b.scope == scope
            && !key.modifiers.intersects(CTRL | KeyModifiers::ALT)
    })
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
/// Two-key sequence bindings (e.g. `D l`) work through
/// [`crate::state::UiState::pending_prefix`]: a prefix key arms the pending
/// state and is consumed; the next key is first looked up as the sequence
/// continuation, then as a normal binding, so a mistyped continuation never
/// strands the handler in prefix mode.
pub fn handle_session_key(
    session: &mut SessionView,
    key: KeyEvent,
) -> Option<SessionAction> {
    let scope = if session.ui.mode.is_editing() {
        KeyScope::Editing
    } else {
        match &session.content {
            SessionContent::Dir(_) => KeyScope::DirNormal,
            SessionContent::Text(_) => KeyScope::TextNormal,
        }
    };
    let cmd = match session.ui.pending_prefix.take() {
        Some(prefix) => {
            lookup_prefixed(scope, prefix, key).or_else(|| lookup(scope, key))
        }
        None => match lookup(scope, key) {
            Some(cmd) => Some(cmd),
            None => {
                if starts_sequence(scope, key) {
                    session.ui.pending_prefix = Some(key.code);
                    return Some(SessionAction::None);
                }
                None
            }
        },
    }?;
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
        Cmd::PlanSync => {
            // Plan sync with the current operation (dry-run). Bound in the
            // dir scope only, so the dir state is present.
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
        Cmd::ExecuteSync => {
            // Execute the planned sync (only meaningful when a preview is
            // active, but we dispatch it and the controller handles it).
            SessionAction::ExecuteSync
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
        Cmd::GoUp => SessionAction::GoUp,
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
        Cmd::CopyRight => transfer_action(session, TransferAction::CopyRight),
        Cmd::CopyLeft => transfer_action(session, TransferAction::CopyLeft),
        Cmd::DeleteLeft => {
            transfer_action(session, TransferAction::DeleteLeft)
        }
        Cmd::DeleteRight => {
            transfer_action(session, TransferAction::DeleteRight)
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

/// Handle Enter key — open the selected directory in a new session.
fn handle_enter(session: &SessionView) -> SessionAction {
    let entries = session.filtered_entries();
    let selected = match session.ui.table_state.selected() {
        Some(i) if i < entries.len() => entries[i],
        _ => return SessionAction::None,
    };

    // Build the sub-endpoints for the new comparison. An entry's `path` is
    // relative to the current comparison root, so it must be joined onto the
    // endpoint's path rather than used as the endpoint path directly (which
    // would drop the root and resolve to a non-existent location). An entry
    // that only exists on one side falls back to the joined name on the other
    // side, which then reads as a non-existent file or directory.
    let left_sub = selected
        .left
        .as_ref()
        .map(|l| session.core.left.join(&l.path))
        .or_else(|| {
            // Entry only exists on the right; use a non-existent left path.
            Some(session.core.left.join(&selected.name))
        });
    let right_sub = selected
        .right
        .as_ref()
        .map(|r| session.core.right.join(&r.path))
        .or_else(|| {
            // Entry only exists on the left; use a non-existent right path.
            Some(session.core.right.join(&selected.name))
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
    let ops = SyncOperation::ALL;
    let len = ops.len() as isize;
    let idx = ops.iter().position(|x| x == current).unwrap_or(0) as isize;
    let new_idx = ((idx + delta) % len + len) % len;
    ops[new_idx as usize]
}

/// Resolve the selected entry to a transfer action for `action`.
///
/// An entry that does not exist on the side the action reads from or
/// removes (e.g. copying left → right an entry that only exists on the
/// right) is a no-op.
fn transfer_action(
    session: &SessionView,
    action: TransferAction,
) -> SessionAction {
    match selected_transfer_item(session, action) {
        Some(item) => SessionAction::Transfer { item },
        None => SessionAction::None,
    }
}

/// Build the [`TransferItem`] for the selected entry and `action`.
///
/// Copies take their content from the source side, deletes remove that
/// side's own copy. The item carries the entry's path relative to the
/// comparison root, exactly as the executor expects it.
fn selected_transfer_item(
    session: &SessionView,
    action: TransferAction,
) -> Option<TransferItem> {
    let entries = session.filtered_entries();
    let selected = session
        .ui
        .table_state
        .selected()
        .filter(|&i| i < entries.len())
        .map(|i| entries[i])?;
    let info = match action {
        TransferAction::CopyRight | TransferAction::DeleteLeft => {
            selected.left.as_ref()?
        }
        TransferAction::CopyLeft | TransferAction::DeleteRight => {
            selected.right.as_ref()?
        }
        _ => return None,
    };
    Some(TransferItem::new(
        action,
        selected.name.clone(),
        info.is_dir,
        info.path.clone(),
    ))
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use cocomo_lib::{
        DirComparison, DirEntry, DirEntryStatus, Endpoint, EntryInfo, Grammar,
        LineInfo, TextDiff, TextDifference,
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

    fn press_key(
        session: &mut SessionView,
        key: KeyEvent,
    ) -> Option<SessionAction> {
        handle_session_key(session, key)
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
    fn ctrl_l_builds_copy_item_from_selected_entry() {
        let mut session = dir_session_with_sides();
        session.ui.table_state.select(Some(0));
        let ctrl_l = KeyEvent::new(KeyCode::Char('l'), CTRL);
        match press_key(&mut session, ctrl_l) {
            Some(SessionAction::Transfer { item }) => {
                assert_eq!(item.action, TransferAction::CopyRight);
                assert_eq!(item.name, "left-only.txt");
                assert_eq!(item.rel_path, "left-only.txt");
                assert!(!item.is_dir);
            }
            other => panic!("expected Transfer, got {other:?}"),
        }
    }

    #[test]
    fn copy_without_source_side_is_a_noop() {
        // Copying left → right an entry that only exists on the right does
        // nothing (the key is still owned, so it cannot leak to siblings).
        let mut session = dir_session_with_sides();
        session.ui.table_state.select(Some(1));
        let ctrl_l = KeyEvent::new(KeyCode::Char('l'), CTRL);
        assert_eq!(press_key(&mut session, ctrl_l), Some(SessionAction::None));
    }

    #[test]
    fn d_prefix_sequence_deletes() {
        let mut session = dir_session_with_sides();
        session.ui.table_state.select(Some(0));

        // The prefix alone arms the pending state and is consumed.
        assert_eq!(
            press(&mut session, KeyCode::Char('D')),
            Some(SessionAction::None)
        );
        assert_eq!(session.ui.pending_prefix, Some(KeyCode::Char('D')));

        // The continuation resolves to the delete action and clears the
        // pending state.
        match press(&mut session, KeyCode::Char('l')) {
            Some(SessionAction::Transfer { item }) => {
                assert_eq!(item.action, TransferAction::DeleteLeft);
                assert_eq!(item.name, "left-only.txt");
            }
            other => panic!("expected Transfer, got {other:?}"),
        }
        assert_eq!(session.ui.pending_prefix, None);

        // `D r` deletes on the right side.
        session.ui.table_state.select(Some(1));
        press(&mut session, KeyCode::Char('D'));
        match press(&mut session, KeyCode::Char('r')) {
            Some(SessionAction::Transfer { item }) => {
                assert_eq!(item.action, TransferAction::DeleteRight);
                assert_eq!(item.name, "right-only.txt");
            }
            other => panic!("expected Transfer, got {other:?}"),
        }
    }

    #[test]
    fn d_prefix_aborts_on_unbound_continuation() {
        let mut session = dir_session_with_sides();
        press(&mut session, KeyCode::Char('D'));
        // An unbound continuation clears the pending prefix and stays
        // unbound itself.
        assert_eq!(press(&mut session, KeyCode::Char('x')), None);
        assert_eq!(session.ui.pending_prefix, None);
        // Bare `r` is Reload again, not a swallowed `D r`.
        assert_eq!(
            press(&mut session, KeyCode::Char('r')),
            Some(SessionAction::Reload)
        );
    }

    #[test]
    fn s_plans_sync_like_p() {
        // Until the confirmation modal lands, `S` plans the sync with the
        // currently configured operation.
        let mut session = dir_session();
        assert_eq!(
            press(&mut session, KeyCode::Char('S')),
            Some(SessionAction::PlanSync {
                operation: SyncOperation::default()
            })
        );
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
