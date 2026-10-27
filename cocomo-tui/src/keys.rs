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
//! bindings to the two session views, [`KeyScope::Menu`] to an open menu
//! dropdown, [`KeyScope::Dialog`] to the open
//! transfer dialog, [`KeyScope::FilterPanel`], [`KeyScope::SessionBrowser`],
//! [`KeyScope::Settings`] and [`KeyScope::Report`] to the modal panels,
//! and [`KeyScope::Editing`] is the root-layer fallback while a text-entry
//! mode is active.
//!
//! [`handle_session_key`] is pure with respect to the global state: it
//! mutates only the [`SessionView`] passed in and reports key ownership
//! through its return value — `None` means the key is unbound, so the
//! caller can return `EventResult::Ignored` instead of swallowing the key.
//! Rendering is done by the ratatui-kit components in `ui/`.

use std::path::PathBuf;

use cocomo_lib::{SyncOperation, TransferItem};
use ratatui_kit::{
    components::tui_input::{Input, InputRequest},
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
    ratatui::{
        style::{Modifier, Style},
        text::{Line, Span},
    },
};

use crate::{
    diff_model::diff_group_starts,
    state::{
        FilterDraft, FilterFocus, NamePattern, ReportDraft, ReportFocus,
        ReportSelection, SessionAction, SessionContent, SessionMode,
        SessionView, StatusFilter, TransferChoice, TransferOp,
    },
};

/// Shorthand for an unmodified key binding.
const NONE: KeyModifiers = KeyModifiers::NONE;
/// Shorthand for a Ctrl binding.
const CTRL: KeyModifiers = KeyModifiers::CONTROL;
/// Shorthand for an Alt binding.
const ALT: KeyModifiers = KeyModifiers::ALT;

/// Which handler owns a binding.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum KeyScope {
    /// The App shell: tab management, menu navigation, quit.
    Global,
    /// A directory-comparison session in normal mode.
    DirNormal,
    /// A text-comparison session in normal mode.
    TextNormal,
    /// A menu dropdown is open. The App shell routes keys here first
    /// while [`crate::state::AppState::menu`] is `Some`; the navigation
    /// keys live in this table so they stay documented, and the shell
    /// claims every other key so lower handlers cannot react behind the
    /// dropdown.
    Menu,
    /// Any session while a text-entry mode is active. The input components
    /// capture keys in their own layers; this scope is only the root-layer
    /// fallback that cancels the mode.
    Editing,
    /// The transfer dialog is open. Its accelerators live here so they stay
    /// documented in the table; navigation and `Enter` are owned by the
    /// dialog's `Select` component and deliberately not listed.
    Dialog,
    /// The filter panel modal is open. `Enter` applies, `Tab` moves the
    /// focus; keys the focused child widget owns (text editing, `Space` on
    /// the status list) are deliberately not listed.
    FilterPanel,
    /// The session browser modal is open. `j`/`k`/`Enter` are owned by its
    /// `Select` child and deliberately not listed.
    SessionBrowser,
    /// The settings modal is open. `Enter` toggles the highlighted entry
    /// through its `Select` child and is deliberately not listed.
    Settings,
    /// The report modal is open. `Enter` generates from every section but
    /// the format list (where the `Select` child owns it) and is handled
    /// dynamically, so it is deliberately not listed.
    Report,
}

/// The command a [`KeyBinding`] performs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Cmd {
    // -- Global scope: executed by the App shell. --
    /// Close the active tab (quit when it was the last one).
    CloseTab,
    /// Switch to the next tab, or the previous one with Shift.
    SwitchTab,
    /// Open the menu at the given bar index (`Alt+<accel>`; a test keeps
    /// the bindings in sync with [`crate::menu::MENUS`]).
    MenuOpen(usize),
    /// Select the previous menu on the bar.
    MenuPrev,
    /// Select the next menu on the bar.
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
    /// Open the filter panel modal (`f`).
    OpenFilterPanel,
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
    /// Select the given sync operation (dropdown item).
    SetSyncOp(SyncOperation),
    /// Toggle the visibility of one entry-status class (dropdown item).
    ToggleStatusFilter(StatusFilter),
    /// Enter save-session mode.
    StartSaveSession,
    /// Open the session browser modal and list saved sessions (`O`).
    OpenSessionBrowser,
    /// Open the settings modal (`,`).
    OpenSettings,
    /// Open the report modal (`R`).
    OpenReport,
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
    // -- Menu scope: executed by the App shell while a menu is open. --
    /// Highlight the previous item in the open dropdown (`k`).
    MenuUp,
    /// Highlight the next item in the open dropdown (`j`).
    MenuDown,
    /// Run the highlighted dropdown item (`Enter`).
    MenuConfirm,
    /// Close the open dropdown without running anything (`Esc`).
    MenuClose,
    // -- Modal panel scopes: executed by the panel handlers. --
    /// Close an open modal panel without applying it.
    PanelClose,
    /// Move a modal panel's focus to the next section (`Tab`).
    PanelFocusNext,
    /// Apply the filter panel draft (`Enter`).
    PanelApply,
    /// Toggle the filter panel's name-filter enable flag (`Space`).
    PanelToggle,
    /// Close the session browser and enter save mode to create a new
    /// session (`n`).
    SessionNew,
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
        cmd: Cmd::OpenFilterPanel,
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
        cmd: Cmd::OpenSessionBrowser,
        help: Some(("O", "sessions")),
    },
    KeyBinding {
        key: KeyCode::Char(','),
        modifiers: NONE,
        scope: KeyScope::DirNormal,
        cmd: Cmd::OpenSettings,
        help: Some((",", "settings")),
    },
    KeyBinding {
        key: KeyCode::Char('R'),
        modifiers: NONE,
        scope: KeyScope::DirNormal,
        cmd: Cmd::OpenReport,
        help: Some(("R", "report")),
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
    // -- Filter panel modal (its own input layer; the focused child widget
    //    owns the keys it needs, so only the panel-level keys are listed). --
    KeyBinding {
        key: KeyCode::Esc,
        modifiers: NONE,
        scope: KeyScope::FilterPanel,
        cmd: Cmd::PanelClose,
        help: None,
    },
    KeyBinding {
        key: KeyCode::Tab,
        modifiers: NONE,
        scope: KeyScope::FilterPanel,
        cmd: Cmd::PanelFocusNext,
        help: None,
    },
    KeyBinding {
        key: KeyCode::Enter,
        modifiers: NONE,
        scope: KeyScope::FilterPanel,
        cmd: Cmd::PanelApply,
        help: None,
    },
    KeyBinding {
        key: KeyCode::Char(' '),
        modifiers: NONE,
        scope: KeyScope::FilterPanel,
        cmd: Cmd::PanelToggle,
        help: None,
    },
    // -- Session browser modal. --
    KeyBinding {
        key: KeyCode::Esc,
        modifiers: NONE,
        scope: KeyScope::SessionBrowser,
        cmd: Cmd::PanelClose,
        help: None,
    },
    KeyBinding {
        key: KeyCode::Char('n'),
        modifiers: NONE,
        scope: KeyScope::SessionBrowser,
        cmd: Cmd::SessionNew,
        help: None,
    },
    // -- Settings modal. --
    KeyBinding {
        key: KeyCode::Esc,
        modifiers: NONE,
        scope: KeyScope::Settings,
        cmd: Cmd::PanelClose,
        help: None,
    },
    // -- Report modal. --
    KeyBinding {
        key: KeyCode::Esc,
        modifiers: NONE,
        scope: KeyScope::Report,
        cmd: Cmd::PanelClose,
        help: None,
    },
    KeyBinding {
        key: KeyCode::Tab,
        modifiers: NONE,
        scope: KeyScope::Report,
        cmd: Cmd::PanelFocusNext,
        help: None,
    },
    // -- Menu dropdown (open; the App shell routes keys here first and
    //    claims everything else, so only the navigation keys are listed). --
    KeyBinding {
        key: KeyCode::Char('j'),
        modifiers: NONE,
        scope: KeyScope::Menu,
        cmd: Cmd::MenuDown,
        help: None,
    },
    KeyBinding {
        key: KeyCode::Down,
        modifiers: NONE,
        scope: KeyScope::Menu,
        cmd: Cmd::MenuDown,
        help: None,
    },
    KeyBinding {
        key: KeyCode::Char('k'),
        modifiers: NONE,
        scope: KeyScope::Menu,
        cmd: Cmd::MenuUp,
        help: None,
    },
    KeyBinding {
        key: KeyCode::Up,
        modifiers: NONE,
        scope: KeyScope::Menu,
        cmd: Cmd::MenuUp,
        help: None,
    },
    KeyBinding {
        key: KeyCode::Enter,
        modifiers: NONE,
        scope: KeyScope::Menu,
        cmd: Cmd::MenuConfirm,
        help: None,
    },
    KeyBinding {
        key: KeyCode::Esc,
        modifiers: NONE,
        scope: KeyScope::Menu,
        cmd: Cmd::MenuClose,
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
    // -- Alt accelerators: one per menu, opening its dropdown directly.
    //    A test keeps these in sync with `menu::MENUS`; they carry no help
    //    label because the menu bar itself renders the accels. --
    KeyBinding {
        key: KeyCode::Char('s'),
        modifiers: ALT,
        scope: KeyScope::Global,
        cmd: Cmd::MenuOpen(0),
        help: None,
    },
    KeyBinding {
        key: KeyCode::Char('c'),
        modifiers: ALT,
        scope: KeyScope::Global,
        cmd: Cmd::MenuOpen(1),
        help: None,
    },
    KeyBinding {
        key: KeyCode::Char('y'),
        modifiers: ALT,
        scope: KeyScope::Global,
        cmd: Cmd::MenuOpen(2),
        help: None,
    },
    KeyBinding {
        key: KeyCode::Char('v'),
        modifiers: ALT,
        scope: KeyScope::Global,
        cmd: Cmd::MenuOpen(3),
        help: None,
    },
    KeyBinding {
        key: KeyCode::Char('f'),
        modifiers: ALT,
        scope: KeyScope::Global,
        cmd: Cmd::MenuOpen(4),
        help: None,
    },
    KeyBinding {
        key: KeyCode::Char('h'),
        modifiers: ALT,
        scope: KeyScope::Global,
        cmd: Cmd::MenuOpen(5),
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

/// Return the help-bar key label of `cmd`, if a labelled binding runs it.
///
/// Menu items use this to display the keyboard counterpart of their
/// command, so the dropdown never carries a second label table that could
/// drift from the keymap.
pub fn key_hint(cmd: Cmd) -> Option<&'static str> {
    KEYMAP
        .iter()
        .find(|b| b.cmd == cmd && b.help.is_some())?
        .help
        .map(|(keys, _)| keys)
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
/// though the dialog renders in its own input layer. The modal panels
/// (`FilterPanel`, `SessionBrowser`, `Settings`, `Report`) route to
/// dedicated handlers because their key ownership depends on which panel
/// section currently has focus.
pub fn handle_session_key(
    session: &mut SessionView,
    key: KeyEvent,
) -> Option<SessionAction> {
    match session.ui.mode {
        SessionMode::FilterPanel { .. } => {
            return handle_filter_panel_key(session, key);
        }
        SessionMode::SessionBrowser { .. } => {
            return handle_browser_key(session, key);
        }
        SessionMode::Settings => return handle_settings_key(session, key),
        SessionMode::Report { .. } => return handle_report_key(session, key),
        SessionMode::SyncConfirm => {
            // The sync confirmation modal owns every key while it is open;
            // its exclusive input layer normally blocks this handler
            // already, and claiming nothing here is the belt-and-braces
            // counterpart.
            return None;
        }
        _ => {}
    }
    let scope = if session.ui.mode.is_editing() {
        KeyScope::Editing
    } else if matches!(session.ui.mode, SessionMode::Transfer { .. }) {
        KeyScope::Dialog
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
            session.set_cursor(Some(0));
            SessionAction::None
        }
        Cmd::LastRow => {
            let count = session.row_count();
            if count > 0 {
                session.set_cursor(Some(count - 1));
            }
            SessionAction::None
        }
        Cmd::OpenFilterPanel => {
            // Seed the draft from the session's current filters so the
            // panel always shows the effective state.
            let mut draft = FilterDraft::default();
            if let Some(dir) = session.dir() {
                draft.show_same = dir.display.show_same;
                draft.show_different = dir.display.show_different;
                draft.show_orphans = dir.display.show_orphans;
            }
            draft.name_filter_on = session.ui.name_filter.is_some();
            if let Some(filter) = &session.ui.name_filter {
                draft.pattern = filter.pattern.clone();
                draft.cursor = filter.pattern.chars().count();
            }
            session.ui.mode = SessionMode::FilterPanel {
                draft: Box::new(draft),
            };
            SessionAction::None
        }
        Cmd::ToggleHideSame => {
            if let Some(dir) = session.dir_mut() {
                dir.display.show_same = !dir.display.show_same;
            }
            session.set_cursor(Some(0));
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
        Cmd::SetSyncOp(operation) => {
            if let Some(dir) = session.dir_mut() {
                dir.sync_operation = operation;
            }
            SessionAction::None
        }
        Cmd::ToggleStatusFilter(status) => {
            if let Some(dir) = session.dir_mut() {
                let show = match status {
                    StatusFilter::Same => &mut dir.display.show_same,
                    StatusFilter::Different => &mut dir.display.show_different,
                    StatusFilter::Orphans => &mut dir.display.show_orphans,
                };
                *show = !*show;
            }
            session.set_cursor(Some(0));
            SessionAction::None
        }
        Cmd::StartSaveSession => {
            // Enter input mode; the name buffer lives in the mode itself.
            session.ui.mode = SessionMode::SaveSession {
                name: String::new(),
            };
            SessionAction::None
        }
        Cmd::OpenSessionBrowser => {
            // Open the browser and list saved sessions for it.
            session.ui.mode = SessionMode::SessionBrowser {
                saved_sessions: Vec::new(),
            };
            SessionAction::ListSessions
        }
        Cmd::OpenSettings => {
            session.ui.mode = SessionMode::Settings;
            SessionAction::None
        }
        Cmd::OpenReport => {
            session.ui.mode = SessionMode::Report {
                draft: Box::new(ReportDraft::default()),
            };
            SessionAction::None
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
            // name filter.
            let cancelled_sync = session
                .dir_mut()
                .is_some_and(|dir| dir.sync.take().is_some());
            if !cancelled_sync {
                session.ui.name_filter = None;
            }
            session.set_cursor(Some(0));
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
            SessionAction::None
        }
        Cmd::PanelClose => {
            session.ui.mode = SessionMode::Normal;
            SessionAction::None
        }
        Cmd::SessionNew => {
            // The browser hands over to the save-name prompt.
            session.ui.mode = SessionMode::SaveSession {
                name: String::new(),
            };
            SessionAction::None
        }
        // These depend on which panel draft is open and are executed by
        // the panel handlers, never through this match.
        Cmd::PanelFocusNext | Cmd::PanelApply | Cmd::PanelToggle => {
            SessionAction::None
        }
        // Global and menu commands are dispatched by the App shell, never
        // here.
        Cmd::CloseTab
        | Cmd::SwitchTab
        | Cmd::MenuOpen(_)
        | Cmd::MenuPrev
        | Cmd::MenuNext
        | Cmd::MenuUp
        | Cmd::MenuDown
        | Cmd::MenuConfirm
        | Cmd::MenuClose
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
                session.set_cursor(Some(selected + 1));
            }
        } else if rows > 0 {
            session.set_cursor(Some(0));
        }
    } else if let Some(selected) =
        session.ui.table_state.selected().filter(|s| *s > 0)
    {
        session.set_cursor(Some(selected - 1));
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
    session.set_cursor(Some(target.unwrap_or(fallback)));
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
        session.set_cursor(Some(index));
    }
}

/// Cycle through sync operations.
pub(crate) fn cycle_sync_op(
    current: &SyncOperation,
    delta: isize,
) -> SyncOperation {
    let ops = SyncOperation::ALL;
    let len = ops.len() as isize;
    let idx = ops.iter().position(|x| x == current).unwrap_or(0) as isize;
    let new_idx = ((idx + delta) % len + len) % len;
    ops[new_idx as usize]
}

/// Open the transfer dialog for the current selection.
///
/// A no-op when nothing is selected (or no selected entry exists on
/// either side), so `c`/`d` on an empty table simply do nothing.
fn open_dialog(session: &mut SessionView, op: TransferOp) -> SessionAction {
    if !session.transfer_targets().is_empty() {
        session.ui.mode = SessionMode::Transfer { op };
    }
    SessionAction::None
}

/// Resolve a transfer-dialog option into the action the controller must
/// dispatch.
///
/// The targets are resolved here rather than trusted from the dialog's
/// render, so an accelerator for an option no target in the selection
/// activates is absorbed: the dialog stays open and nothing transfers.
/// An active option transfers it on every target that supports it. Only an
/// active option (or `abort`) closes the dialog.
pub fn dialog_choice(
    session: &mut SessionView,
    choice: TransferChoice,
) -> SessionAction {
    let SessionMode::Transfer { op } = session.ui.mode else {
        return SessionAction::None;
    };
    let targets = session.transfer_targets();
    if targets.is_empty() {
        // The dialog cannot render without a target; drop the stale mode.
        session.ui.mode = SessionMode::Normal;
        return SessionAction::None;
    }
    // The option activates when any target supports it; `items` then
    // contributes nothing for the targets that do not.
    let has_left = targets.iter().any(|t| t.has_left);
    let has_right = targets.iter().any(|t| t.has_right);
    if !choice.is_enabled(op, has_left, has_right) {
        return SessionAction::None;
    }
    session.ui.mode = SessionMode::Normal;
    let items: Vec<TransferItem> = targets
        .iter()
        .flat_map(|target| target.items(op, choice))
        .collect();
    if items.is_empty() {
        SessionAction::None
    } else {
        SessionAction::Transfer { items }
    }
}

// ---------------------------------------------------------------------------
// Modal panel handlers
// ---------------------------------------------------------------------------

/// Handle a key for the open filter panel modal.
///
/// The panel owns `Esc` (close), `Tab` (focus), `Enter` (apply) and
/// `Space` (name-filter toggle) through the [`KeyScope::FilterPanel`]
/// bindings; everything else reaches the focused section: printable keys
/// edit the pattern buffer while it has focus, and `Space`/`j`/`k` pass
/// through to the status `MultiSelect` while it has focus (returning
/// `None` lets the child in the same input layer see the key).
fn handle_filter_panel_key(
    session: &mut SessionView,
    key: KeyEvent,
) -> Option<SessionAction> {
    let focus = match &session.ui.mode {
        SessionMode::FilterPanel { draft } => draft.focus,
        _ => return None,
    };
    if let Some(cmd) = lookup(KeyScope::FilterPanel, key) {
        // While the status list has focus, `Space` toggles its rows; the
        // `MultiSelect` child must see the key, so the panel does not
        // claim the toggle.
        if cmd == Cmd::PanelToggle && focus == FilterFocus::Statuses {
            return None;
        }
        return Some(match cmd {
            Cmd::PanelClose => {
                session.ui.mode = SessionMode::Normal;
                SessionAction::None
            }
            Cmd::PanelFocusNext => {
                if let SessionMode::FilterPanel { draft } =
                    &mut session.ui.mode
                {
                    draft.focus = next_filter_focus(draft.focus);
                }
                SessionAction::None
            }
            Cmd::PanelToggle => {
                if let SessionMode::FilterPanel { draft } =
                    &mut session.ui.mode
                {
                    draft.name_filter_on = !draft.name_filter_on;
                }
                SessionAction::None
            }
            Cmd::PanelApply => apply_filter_draft(session),
            _ => SessionAction::None,
        });
    }
    // While the pattern field has focus, editing keys mutate the buffer.
    if focus == FilterFocus::Pattern
        && let SessionMode::FilterPanel { draft } = &mut session.ui.mode
        && edit_text(&mut draft.pattern, &mut draft.cursor, key)
    {
        draft.error = None;
        return Some(SessionAction::None);
    }
    None
}

/// Handle a key for the open session browser modal.
///
/// The browser owns `Esc` (close) and `n` (create new); `j`/`k`/`Enter`
/// pass through to its `Select` child, which loads the highlighted
/// session.
fn handle_browser_key(
    session: &mut SessionView,
    key: KeyEvent,
) -> Option<SessionAction> {
    if !matches!(session.ui.mode, SessionMode::SessionBrowser { .. }) {
        return None;
    }
    let cmd = lookup(KeyScope::SessionBrowser, key)?;
    Some(execute(cmd, session))
}

/// Handle a key for the open settings modal.
///
/// The modal owns only `Esc`; `Enter` toggles the highlighted entry
/// through its `Select` child.
fn handle_settings_key(
    session: &mut SessionView,
    key: KeyEvent,
) -> Option<SessionAction> {
    if !matches!(session.ui.mode, SessionMode::Settings) {
        return None;
    }
    let cmd = lookup(KeyScope::Settings, key)?;
    Some(execute(cmd, session))
}

/// Handle a key for the open report modal.
///
/// `Enter` generates the report from every section except the format list
/// (where the `Select` child owns it to confirm the highlighted format);
/// `Esc` closes and `Tab` moves the focus; printable keys edit the
/// destination path while it has focus.
fn handle_report_key(
    session: &mut SessionView,
    key: KeyEvent,
) -> Option<SessionAction> {
    let focus = match &session.ui.mode {
        SessionMode::Report { draft } => draft.focus,
        _ => return None,
    };
    if key.code == KeyCode::Enter && focus != ReportFocus::Format {
        return Some(generate_report_action(session));
    }
    if let Some(cmd) = lookup(KeyScope::Report, key) {
        return Some(match cmd {
            Cmd::PanelClose => {
                session.ui.mode = SessionMode::Normal;
                SessionAction::None
            }
            Cmd::PanelFocusNext => {
                if let SessionMode::Report { draft } = &mut session.ui.mode {
                    draft.focus = next_report_focus(draft.focus);
                }
                SessionAction::None
            }
            _ => SessionAction::None,
        });
    }
    if focus == ReportFocus::Destination
        && let SessionMode::Report { draft } = &mut session.ui.mode
        && edit_text(&mut draft.destination, &mut draft.cursor, key)
    {
        draft.error = None;
        return Some(SessionAction::None);
    }
    None
}

/// Apply the filter panel draft to the session and close the panel.
///
/// An invalid regex keeps the panel open and surfaces the compiler
/// message in the draft instead of silently dropping the filter.
fn apply_filter_draft(session: &mut SessionView) -> SessionAction {
    let (compiled, statuses) = match &session.ui.mode {
        SessionMode::FilterPanel { draft } => {
            let compiled = if draft.name_filter_on && !draft.pattern.is_empty()
            {
                Some(NamePattern::new(&draft.pattern))
            } else {
                None
            };
            (
                compiled,
                (draft.show_same, draft.show_different, draft.show_orphans),
            )
        }
        _ => return SessionAction::None,
    };
    let name_filter = match compiled {
        Some(Ok(pattern)) => Some(pattern),
        Some(Err(e)) => {
            if let SessionMode::FilterPanel { draft } = &mut session.ui.mode {
                draft.error = Some(format!("Invalid pattern: {e}"));
            }
            return SessionAction::None;
        }
        None => None,
    };
    session.ui.name_filter = name_filter;
    let (show_same, show_different, show_orphans) = statuses;
    if let Some(dir) = session.dir_mut() {
        dir.display.show_same = show_same;
        dir.display.show_different = show_different;
        dir.display.show_orphans = show_orphans;
    }
    session.ui.mode = SessionMode::Normal;
    session.set_cursor(Some(0));
    SessionAction::None
}

/// Run a session-scoped [`Cmd`] activated from the menu bar.
///
/// The menu items map onto the same commands the keybindings execute, so a
/// click and its keyboard shortcut cannot drift apart. The controller
/// dispatches the returned action like any key-derived one.
pub fn run_menu_command(session: &mut SessionView, cmd: Cmd) -> SessionAction {
    execute(cmd, session)
}

/// Build the report action from the modal draft.
///
/// An empty destination keeps the modal open with a validation message;
/// otherwise the draft converts to a `ReportConfig` and the controller
/// runs the generation.
fn generate_report_action(session: &mut SessionView) -> SessionAction {
    let (format, config, destination) = match &session.ui.mode {
        SessionMode::Report { draft } => (
            draft.format,
            draft.config(),
            draft.destination.trim().to_string(),
        ),
        _ => return SessionAction::None,
    };
    if destination.is_empty() {
        if let SessionMode::Report { draft } = &mut session.ui.mode {
            draft.error = Some("Enter an output path.".to_string());
        }
        return SessionAction::None;
    }
    SessionAction::GenerateReport {
        format,
        config: ReportSelection(config),
        destination: PathBuf::from(destination),
    }
}

/// Move the filter panel focus to the next section.
fn next_filter_focus(focus: FilterFocus) -> FilterFocus {
    match focus {
        FilterFocus::Pattern => FilterFocus::NameToggle,
        FilterFocus::NameToggle => FilterFocus::Statuses,
        FilterFocus::Statuses => FilterFocus::Pattern,
    }
}

/// Move the report modal focus to the next section.
fn next_report_focus(focus: ReportFocus) -> ReportFocus {
    match focus {
        ReportFocus::Format => ReportFocus::Destination,
        ReportFocus::Destination => ReportFocus::Options,
        ReportFocus::Options => ReportFocus::Format,
    }
}

/// Forward `key` to a `tui_input`-style editor over `value`/`cursor`.
///
/// Returns `true` when the key is an editing key (so the caller reports it
/// as consumed); anything else leaves the buffer untouched and reports
/// `false` so the key can reach a child component in the same layer.
fn edit_text(value: &mut String, cursor: &mut usize, key: KeyEvent) -> bool {
    let mut input = Input::new(std::mem::take(value)).with_cursor(*cursor);
    let request = match key.code {
        KeyCode::Char(c)
            if key.modifiers.is_empty()
                || key.modifiers == KeyModifiers::SHIFT =>
        {
            InputRequest::InsertChar(c)
        }
        KeyCode::Backspace => InputRequest::DeletePrevChar,
        KeyCode::Delete => InputRequest::DeleteNextChar,
        KeyCode::Left => InputRequest::GoToPrevChar,
        KeyCode::Right => InputRequest::GoToNextChar,
        KeyCode::Home => InputRequest::GoToStart,
        KeyCode::End => InputRequest::GoToEnd,
        _ => {
            *value = input.value().to_string();
            return false;
        }
    };
    input.handle(request);
    *value = input.value().to_string();
    *cursor = input.cursor();
    true
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
    fn keyboard_navigation_clears_the_drag_anchor() {
        let mut session = dir_session();
        session.ui.table_state.select(Some(1));
        session.ui.selection_anchor = Some(0);

        press(&mut session, KeyCode::Char('j'));
        assert_eq!(session.ui.selection_anchor, None);

        session.ui.selection_anchor = Some(0);
        press(&mut session, KeyCode::Char('G'));
        assert_eq!(session.ui.selection_anchor, None);

        session.ui.selection_anchor = Some(0);
        press(&mut session, KeyCode::Esc);
        assert_eq!(session.ui.selection_anchor, None);
    }

    #[test]
    fn run_menu_command_executes_session_commands() {
        let mut session = dir_session();
        // `View` toggles hiding identical entries and resets the cursor.
        session.ui.table_state.select(Some(2));
        let action = run_menu_command(&mut session, Cmd::ToggleHideSame);
        assert!(matches!(action, SessionAction::None));
        assert!(!session.dir().unwrap().display.show_same);
        assert_eq!(session.ui.table_state.selected(), Some(0));

        // `Sessions` opens the browser and asks for the saved list.
        let action = run_menu_command(&mut session, Cmd::OpenSessionBrowser);
        assert!(matches!(action, SessionAction::ListSessions));
        assert!(matches!(
            session.ui.mode,
            SessionMode::SessionBrowser { .. }
        ));
    }

    #[test]
    fn set_sync_op_selects_the_operation() {
        let mut session = dir_session();
        let action = run_menu_command(
            &mut session,
            Cmd::SetSyncOp(SyncOperation::MirrorRight),
        );
        assert!(matches!(action, SessionAction::None));
        assert_eq!(
            session.dir().unwrap().sync_operation,
            SyncOperation::MirrorRight
        );
    }

    #[test]
    fn toggle_status_filter_flips_display_flags() {
        let mut session = dir_session();
        session.ui.table_state.select(Some(2));
        run_menu_command(
            &mut session,
            Cmd::ToggleStatusFilter(StatusFilter::Same),
        );
        assert!(!session.dir().unwrap().display.show_same);
        run_menu_command(
            &mut session,
            Cmd::ToggleStatusFilter(StatusFilter::Orphans),
        );
        assert!(!session.dir().unwrap().display.show_orphans);
        assert!(session.dir().unwrap().display.show_different);
        // Like the other filter toggles, the cursor resets.
        assert_eq!(session.ui.table_state.selected(), Some(0));
    }

    #[test]
    fn menu_scope_owns_only_navigation_keys() {
        assert_eq!(
            lookup(KeyScope::Menu, key(KeyCode::Char('j'))),
            Some(Cmd::MenuDown)
        );
        assert_eq!(
            lookup(KeyScope::Menu, key(KeyCode::Down)),
            Some(Cmd::MenuDown)
        );
        assert_eq!(
            lookup(KeyScope::Menu, key(KeyCode::Char('k'))),
            Some(Cmd::MenuUp)
        );
        assert_eq!(
            lookup(KeyScope::Menu, key(KeyCode::Up)),
            Some(Cmd::MenuUp)
        );
        assert_eq!(
            lookup(KeyScope::Menu, key(KeyCode::Enter)),
            Some(Cmd::MenuConfirm)
        );
        assert_eq!(
            lookup(KeyScope::Menu, key(KeyCode::Esc)),
            Some(Cmd::MenuClose)
        );
        // The `←`/`→` menu switching stays in the Global scope.
        assert!(lookup(KeyScope::Menu, key(KeyCode::Left)).is_none());
        // Plain session keys belong to the shell's swallow rule, not to
        // this scope.
        assert!(lookup(KeyScope::Menu, key(KeyCode::Char('x'))).is_none());
    }

    #[test]
    fn alt_accelerators_open_every_menu() {
        for (index, menu) in crate::menu::MENUS.iter().enumerate() {
            let key = KeyEvent::new(KeyCode::Char(menu.accel), ALT);
            assert_eq!(
                lookup(KeyScope::Global, key),
                Some(Cmd::MenuOpen(index)),
                "accel {} of menu {} is not bound",
                menu.accel,
                menu.title
            );
        }
    }

    #[test]
    fn alt_keys_never_fire_plain_bindings() {
        // Alt+S opens the Sessions menu instead of entering save mode.
        let alt_s = KeyEvent::new(KeyCode::Char('s'), ALT);
        assert!(lookup(KeyScope::DirNormal, alt_s).is_none());
        assert_eq!(lookup(KeyScope::Global, alt_s), Some(Cmd::MenuOpen(0)));
    }

    #[test]
    fn key_hint_returns_the_labelled_binding() {
        assert_eq!(key_hint(Cmd::OpenSessionBrowser), Some("O"));
        assert_eq!(key_hint(Cmd::ToggleHelp), Some("?"));
        // Dialog accelerators carry no help label, so no hint either.
        assert_eq!(key_hint(Cmd::DialogPick(TransferChoice::Left)), None);
        assert_eq!(key_hint(Cmd::MenuUp), None);
        // Labelled shell keys do report their hint.
        assert_eq!(key_hint(Cmd::MenuPrev), Some("←/→"));
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
        session.ui.mode = SessionMode::Search {
            query: "partial".into(),
        };
        let action = press(&mut session, KeyCode::Esc);
        assert_eq!(action, Some(SessionAction::None));
        assert_eq!(session.ui.mode, SessionMode::Normal);

        // While editing, printable keys are not owned by this handler.
        session.ui.mode = SessionMode::Search {
            query: String::new(),
        };
        assert_eq!(press(&mut session, KeyCode::Char('x')), None);
        // `q` cancels the mode instead of quitting.
        assert!(press(&mut session, KeyCode::Char('q')).is_some());
        assert_eq!(session.ui.mode, SessionMode::Normal);
    }

    #[test]
    fn save_mode_and_browser_carry_their_own_state() {
        let mut session = dir_session();
        session.ui.name_filter = Some(NamePattern::new("keep me").unwrap());

        // Save mode owns its name buffer; the name filter is untouched.
        assert_eq!(
            press(&mut session, KeyCode::Char('s')),
            Some(SessionAction::None)
        );
        assert!(matches!(
            &session.ui.mode,
            SessionMode::SaveSession { name } if name.is_empty()
        ));
        assert_eq!(
            session.ui.name_filter.as_ref().unwrap().pattern,
            "keep me"
        );

        // Cancelling drops the mode with it.
        assert!(press(&mut session, KeyCode::Char('q')).is_some());
        assert_eq!(session.ui.mode, SessionMode::Normal);

        // The browser opens empty and asks the controller to fill the
        // saved-session list.
        assert_eq!(
            press(&mut session, KeyCode::Char('O')),
            Some(SessionAction::ListSessions)
        );
        assert!(matches!(
            &session.ui.mode,
            SessionMode::SessionBrowser { saved_sessions }
                if saved_sessions.is_empty()
        ));
        // `n` hands over to the save-name prompt.
        assert_eq!(
            press(&mut session, KeyCode::Char('n')),
            Some(SessionAction::None)
        );
        assert!(matches!(
            &session.ui.mode,
            SessionMode::SaveSession { name } if name.is_empty()
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
    fn dialog_choice_transfers_every_target_in_the_drag_range() {
        // Range 0..=2 over left-only, right-only, and both: `l` (copy left
        // → right) applies to the two entries that have a left side, and
        // the right-only entry contributes nothing.
        let mut session = dir_session_with_sides();
        session.ui.table_state.select(Some(2));
        session.ui.selection_anchor = Some(0);
        press(&mut session, KeyCode::Char('c'));
        assert!(matches!(
            session.ui.mode,
            SessionMode::Transfer {
                op: TransferOp::Copy
            }
        ));
        match press(&mut session, KeyCode::Char('l')) {
            Some(SessionAction::Transfer { items }) => {
                assert_eq!(items.len(), 2);
                let names: Vec<&str> =
                    items.iter().map(|i| i.name.as_str()).collect();
                assert_eq!(names, vec!["left-only.txt", "both.txt"]);
                assert!(
                    items
                        .iter()
                        .all(|i| i.action == TransferAction::CopyRight)
                );
            }
            other => panic!("expected Transfer, got {other:?}"),
        }
        assert_eq!(session.ui.mode, SessionMode::Normal);
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

    #[test]
    fn filter_panel_opens_seeded_and_applies_its_draft() {
        let mut session = dir_session();
        assert_eq!(
            press(&mut session, KeyCode::Char('f')),
            Some(SessionAction::None)
        );
        let SessionMode::FilterPanel { draft } = &session.ui.mode else {
            panic!("expected FilterPanel mode");
        };
        // The draft mirrors the session's current display filters.
        assert!(draft.show_same);
        assert!(!draft.name_filter_on);

        // Typing edits the pattern buffer while it has focus.
        for c in "\\.txt".chars() {
            press(&mut session, KeyCode::Char(c));
        }
        // `Tab` moves the focus to the name-filter toggle, where typing no
        // longer edits the buffer.
        assert_eq!(
            press(&mut session, KeyCode::Tab),
            Some(SessionAction::None)
        );
        press(&mut session, KeyCode::Char('z'));
        // `Space` toggles the name filter on.
        assert_eq!(
            press(&mut session, KeyCode::Char(' ')),
            Some(SessionAction::None)
        );
        // `Enter` applies the draft and closes the panel.
        assert_eq!(
            press(&mut session, KeyCode::Enter),
            Some(SessionAction::None)
        );
        assert_eq!(session.ui.mode, SessionMode::Normal);
        assert_eq!(session.ui.name_filter.as_ref().unwrap().pattern, "\\.txt");
        assert!(
            session
                .ui
                .name_filter
                .as_ref()
                .unwrap()
                .is_match("file.txt")
        );
        assert!(!session.ui.name_filter.as_ref().unwrap().is_match("sub"));
    }

    #[test]
    fn filter_panel_invalid_regex_stays_open_with_error() {
        let mut session = dir_session();
        press(&mut session, KeyCode::Char('f'));
        press(&mut session, KeyCode::Char('('));
        // The pattern is only compiled once the name filter is enabled.
        press(&mut session, KeyCode::Tab);
        press(&mut session, KeyCode::Char(' '));
        assert_eq!(
            press(&mut session, KeyCode::Enter),
            Some(SessionAction::None)
        );
        let SessionMode::FilterPanel { draft } = &session.ui.mode else {
            panic!("panel must stay open on an invalid pattern");
        };
        assert!(draft.error.is_some());
        assert!(session.ui.name_filter.is_none());
    }

    #[test]
    fn filter_panel_passes_list_keys_to_the_status_multiselect() {
        let mut session = dir_session();
        press(&mut session, KeyCode::Char('f'));
        // Focus: Pattern -> NameToggle -> Statuses.
        press(&mut session, KeyCode::Tab);
        press(&mut session, KeyCode::Tab);
        // While the status list has focus, `Space` and `j` belong to the
        // child component, so the handler reports them unbound.
        assert_eq!(press(&mut session, KeyCode::Char(' ')), None);
        assert_eq!(press(&mut session, KeyCode::Char('j')), None);
        // `Enter` still applies from the list section.
        assert_eq!(
            press(&mut session, KeyCode::Enter),
            Some(SessionAction::None)
        );
        assert_eq!(session.ui.mode, SessionMode::Normal);
    }

    #[test]
    fn settings_modal_opens_and_closes() {
        let mut session = dir_session();
        assert_eq!(
            press(&mut session, KeyCode::Char(',')),
            Some(SessionAction::None)
        );
        assert_eq!(session.ui.mode, SessionMode::Settings);
        // `j` belongs to the `Select` child; the handler does not claim it.
        assert_eq!(press(&mut session, KeyCode::Char('j')), None);
        assert_eq!(
            press(&mut session, KeyCode::Esc),
            Some(SessionAction::None)
        );
        assert_eq!(session.ui.mode, SessionMode::Normal);
    }

    #[test]
    fn report_modal_generates_on_enter_with_a_destination() {
        let mut session = dir_session();
        assert_eq!(
            press(&mut session, KeyCode::Char('R')),
            Some(SessionAction::None)
        );
        // Focus starts on the format list, where `Enter` is owned by the
        // `Select` child.
        assert_eq!(press(&mut session, KeyCode::Enter), None);
        // `Tab` moves to the destination field; typing edits the path.
        press(&mut session, KeyCode::Tab);
        for c in "out.csv".chars() {
            press(&mut session, KeyCode::Char(c));
        }
        // `Enter` from the path field dispatches the generation.
        match press(&mut session, KeyCode::Enter) {
            Some(SessionAction::GenerateReport {
                format,
                config,
                destination,
            }) => {
                assert_eq!(format, cocomo_lib::report::ReportFormat::Text);
                assert!(config.0.include_different);
                assert!(!config.0.include_same);
                assert_eq!(destination, PathBuf::from("out.csv"));
            }
            other => panic!("expected GenerateReport, got {other:?}"),
        }
    }

    #[test]
    fn report_modal_requires_a_destination() {
        let mut session = dir_session();
        press(&mut session, KeyCode::Char('R'));
        press(&mut session, KeyCode::Tab);
        // An empty destination keeps the modal open with a message.
        assert_eq!(
            press(&mut session, KeyCode::Enter),
            Some(SessionAction::None)
        );
        let SessionMode::Report { draft } = &session.ui.mode else {
            panic!("modal must stay open without a destination");
        };
        assert!(draft.error.is_some());
    }
}
