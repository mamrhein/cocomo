// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Directory comparison view component.

use ratatui_kit::{
    crossterm::event::{Event, KeyCode, KeyEventKind},
    prelude::*,
    ratatui::{
        layout::{Constraint, Direction},
        style::{Modifier, Style},
        text::{Line, Span},
    },
};

use super::{
    frame::{HelpBar, StatusBar},
    rows::{CellRow, cell_row_renderer, dir_rows, sync_rows},
    session_table::SessionTable,
    theme::CocomoTheme,
};
use crate::{
    controller::{
        dispatch_dir_key, set_active_mode, set_filter_input, set_mode_name,
        submit_filter, submit_load_name, submit_save_name,
    },
    keys::{self, KeyScope},
    state::{APP_STATE, SessionMode, SyncStage},
};

/// Directory comparison view using ratatui-kit's `Table` component.
///
/// Subscribes to the global [`APP_STATE`] atom for reactive reads and
/// registers an event handler that delegates to the controller
/// (`dispatch_dir_key`); all state mutations go through controller
/// functions. The table rows are memoized view models ([`CellRow`]) that
/// are rebuilt only when the comparison revision, the hide-identical
/// toggle, or the active filter changes. All text entry (filter, save and
/// load names) uses `SearchInput`, which owns its exclusive input layer.
#[component]
pub(super) fn DirCompareView(
    mut hooks: Hooks,
) -> impl Into<AnyElement<'static>> {
    // Subscribe to global state for re-render notifications on writes.
    let app_handle = hooks.use_atom(&APP_STATE);
    let theme = hooks.use_component_theme::<CocomoTheme>();
    // Terminal width drives the help bar wrapping; the hook re-renders on
    // resize. Must stay above the early returns below (stable hook order).
    let (term_width, _) = hooks.use_terminal_size();

    // Read session data from the atom.
    let app = app_handle.read();
    let session = app.active().filter(|s| s.content.is_dir());
    let dir = session.and_then(|s| s.dir());
    let session_selected = session.and_then(|s| s.ui.table_state.selected());

    // Memo keys: the session identity plus the revision counters and the
    // filters that change the visible rows. `DirComparison` has no
    // `PartialEq`, so the counters (bumped by `set_comparison` and
    // `set_sync`) are the change signal.
    let (rows_key, preview_key) = match session {
        Some(s) => match s.dir() {
            Some(d) => (
                Some((
                    s.core.id,
                    d.comparison_revision,
                    d.hide_same,
                    s.ui.active_filter.clone(),
                )),
                d.sync.as_ref().map(|_| (s.core.id, d.sync_revision)),
            ),
            None => (None, None),
        },
        None => (None, None),
    };

    let entry_rows: Vec<CellRow> = hooks.use_memo(
        || match session {
            Some(s) => dir_rows(&s.filtered_entries(), &theme),
            None => Vec::new(),
        },
        rows_key,
    );
    let preview_rows: Vec<CellRow> = hooks.use_memo(
        || match session.and_then(|s| s.dir()).and_then(|d| d.sync.as_ref()) {
            Some(stage) => sync_rows(&stage.result().planned, &theme),
            None => Vec::new(),
        },
        preview_key,
    );

    let Some(session) = session else {
        drop(app);
        return element!(View {});
    };
    // The filter above guarantees a dir session; the binding keeps the
    // borrow checker happy without a panic path.
    let Some(dir) = dir else {
        drop(app);
        return element!(View {});
    };

    // Extract rendering data from the session. The sync status label reads
    // from a single match on the `SyncStage`, so "Planned" vs. "Executed"
    // is decided by the variant, not by inspecting the `SyncResult`.
    let sync_status: Option<String> =
        dir.sync.as_ref().map(|stage| match stage {
            SyncStage::Planned(result) => {
                format!("Planned: {} transfers", result.planned_count())
            }
            SyncStage::Executed(_) => "Executed".to_string(),
        });
    let sync_active = dir.sync.is_some();
    let mode = session.ui.mode.clone();
    let hide_same = dir.hide_same;
    let active_filter = session.ui.active_filter.clone();
    let left_path_str = session.core.left.display().to_string();
    let right_path_str = session.core.right.display().to_string();
    let sync_operation = dir.sync_operation;
    let show_help = app.show_help;

    // Extract data needed after dropping the app handle (for status bar and
    // element! rendering). All session references below use these locals.
    let filter_input = session.ui.filter_input.clone();
    let comparison_stats = dir.comparison.as_ref().map(|c| {
        format!(
            "Total: {}  Same: {}  Diff: {}  Orphans: {}",
            c.total(),
            c.same_count(),
            c.different_count(),
            c.orphan_count(),
        )
    });
    let comparison_is_none = dir.comparison.is_none();

    drop(app);

    // Event handler for session-level keys (j/k, g/G, Enter, f, F, etc.).
    // The App component's global handler runs first and consumes global keys
    // (Ctrl+W, Ctrl+Tab, Left/Right, q). For remaining keys, the controller
    // locks the state briefly, lets `handle_session_key` mutate the session,
    // and dispatches the resulting intent.
    hooks.use_event_handler(
        EventScope::Current,
        EventPriority::Normal,
        move |event| {
            let Event::Key(key) = event else {
                return EventResult::Ignored;
            };
            if key.kind != KeyEventKind::Press {
                return EventResult::Ignored;
            }

            if dispatch_dir_key(key) {
                EventResult::Consumed
            } else {
                EventResult::Ignored
            }
        },
    );

    // -- Build status bar line -----------------------------------------------
    // When in Filter, SaveSession, or LoadSession mode, the status bar is
    // replaced by a SearchInput rendered in the element! tree.

    let mut status_bar_line: Line<'static> =
        if let Some(status_text) = &sync_status {
            let op_label = sync_operation.label();
            Line::from(Span::styled(
                format!("Sync: {op_label}  |  {status_text}"),
                theme.sync_status,
            ))
        } else {
            let stats = comparison_stats;
            let filter_info = active_filter
                .as_ref()
                .map(|f| format!("*{f}*"))
                .unwrap_or_else(|| "No filter".to_string());
            let display = if hide_same {
                "Hiding identical"
            } else {
                "Showing all"
            };
            Line::from(vec![
                Span::raw(stats.unwrap_or_else(|| "No data".to_string())),
                Span::raw("  |  "),
                Span::raw(filter_info),
                Span::raw("  |  "),
                Span::raw(display),
            ])
        };

    // When the help bar is hidden, hint at the toggle key in the status bar
    // so the binding stays discoverable without spending a row on it.
    if !show_help {
        status_bar_line
            .spans
            .push(Span::styled("  ? help", theme.dim));
    }

    // -- Build help bar lines ------------------------------------------------
    // Mode-specific hints stay hand-written (they fit a single line); the
    // normal-mode bindings come from the keymap, wrapped to the terminal
    // width so the bar cannot drift from the actual dispatch and never
    // truncates an item.

    let help_bar_lines: Vec<Line<'static>> = if mode == SessionMode::Filter {
        vec![Line::from("f=activate  Enter=apply  Esc=cancel")]
    } else if matches!(mode, SessionMode::SaveSession { .. }) {
        vec![Line::from("s=activate  Enter=save  Esc=cancel")]
    } else if matches!(mode, SessionMode::LoadSession { .. }) {
        vec![Line::from("O=activate  Enter=load  Esc=cancel")]
    } else if sync_active {
        vec![Line::from(vec![
            Span::styled("j/k", Style::new().add_modifier(Modifier::BOLD)),
            Span::raw(" navigate  "),
            Span::styled("Esc", Style::new().add_modifier(Modifier::BOLD)),
            Span::raw(" cancel"),
        ])]
    } else {
        keys::help_lines(&[KeyScope::DirNormal, KeyScope::Global], term_width)
    };

    // -- Build the element tree ----------------------------------------------

    // Compute the empty-state message before element! (let bindings are not
    // allowed inside element! children blocks).
    let empty_msg = if comparison_is_none {
        "Loading comparison\u{2026}"
    } else if hide_same {
        "No differing entries."
    } else if active_filter.is_some() {
        "No entries match the current filter."
    } else {
        "Directories are identical."
    };

    // The header's path information now lives in the table's border title,
    // so the view spends no rows on a separate header panel.
    let compare_title = format!("{left_path_str} \u{21c4} {right_path_str}");

    element!(
        View(flex_direction: Direction::Vertical, height: Constraint::Fill(1)) {
            // Main content area: sync preview or comparison table.
            if sync_active {
                SessionTable(
                    selected: session_selected,
                    title: "Sync Preview (dry-run)".to_string(),
                    columns: vec![
                        TableColumn::new("Action", Constraint::Length(30)),
                        // `Fill` so the path column absorbs the remaining width.
                        TableColumn::new("Path", Constraint::Fill(1)),
                    ],
                    rows: preview_rows,
                    render_row: Some(cell_row_renderer()),
                    empty_message: Line::from("No transfers planned."),
                )
            } else {
                SessionTable(
                    selected: session_selected,
                    title: compare_title,
                    columns: vec![
                        TableColumn::new("T Name", Constraint::Fill(1)),
                        TableColumn::new(
                            format!("{:>6} {:^17}", "Size", "Modified"),
                            Constraint::Length(24),
                        ),
                        TableColumn::new("S", Constraint::Length(2)),
                        TableColumn::new(
                            format!("{:>6} {:^17}", "Size", "Modified"),
                            Constraint::Length(24),
                        ),
                    ],
                    rows: entry_rows,
                    render_row: Some(cell_row_renderer()),
                    empty_message: Line::from(empty_msg),
                )
            }
            // Status bar or input component depending on mode. All text
            // entry uses SearchInput, which owns its exclusive input layer;
            // the activate key matches the key that entered the mode.
            if mode == SessionMode::Filter {
                SearchInput(
                    value: filter_input,
                    placeholder: "Filter entries...".to_string(),
                    is_editing: true,
                    activate_key: KeyCode::Char('f'),
                    on_change: Handler::from(move |val: String| {
                        set_filter_input(val);
                    }),
                    on_submit: Handler::from(move |val: String| -> bool {
                        submit_filter(val);
                        true
                    }),
                    clear_on_escape: true,
                    on_clear: Handler::from(move |_: ()| {
                        set_active_mode(SessionMode::Normal);
                    }),
                )
            } else if let SessionMode::SaveSession { name } = &mode {
                SearchInput(
                    value: name.clone(),
                    placeholder: "Session name...".to_string(),
                    is_editing: true,
                    activate_key: KeyCode::Char('s'),
                    on_change: Handler::from(move |val: String| {
                        set_mode_name(val);
                    }),
                    on_submit: Handler::from(move |val: String| -> bool {
                        submit_save_name(val)
                    }),
                    clear_on_escape: true,
                    on_clear: Handler::from(move |_: ()| {
                        set_active_mode(SessionMode::Normal);
                    }),
                )
            } else if let SessionMode::LoadSession {
                name,
                saved_sessions,
            } = &mode
            {
                if !saved_sessions.is_empty() {
                    Text(text: Line::from(format!("Saved: {}", saved_sessions.join(", "))))
                }
                SearchInput(
                    value: name.clone(),
                    placeholder: "Session name to load...".to_string(),
                    is_editing: true,
                    activate_key: KeyCode::Char('O'),
                    on_change: Handler::from(move |val: String| {
                        set_mode_name(val);
                    }),
                    on_submit: Handler::from(move |val: String| -> bool {
                        submit_load_name(val)
                    }),
                    clear_on_escape: true,
                    on_clear: Handler::from(move |_: ()| {
                        set_active_mode(SessionMode::Normal);
                    }),
                )
            } else {
                StatusBar(line: status_bar_line)
            }
            // Help bar: only rendered when toggled on (`?`), so the main
            // view area keeps the rows by default.
            if show_help {
                HelpBar(lines: help_bar_lines)
            }
        }
    )
}
