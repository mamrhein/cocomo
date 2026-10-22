// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Directory comparison view component.

use std::sync::Arc;

use cocomo_lib::{DirEntry, TransferAction, TransferItem};
use ratatui_kit::{
    crossterm::event::{Event, KeyCode, KeyEventKind},
    prelude::*,
    ratatui::{
        layout::{Alignment, Constraint, Direction},
        prelude::Stylize,
        style::{Color, Modifier, Style},
        symbols,
        text::{Line, Span},
        widgets::{Block, Borders},
    },
};

use super::{style, use_session_table_state};
use crate::{
    controller::{
        dispatch_session_key, set_active_mode, set_filter_input,
        set_mode_name, submit_filter, submit_load_name, submit_save_name,
    },
    format::{format_date, format_size, status_style},
    keys::{self, KeyScope},
    state::{APP_STATE, Focus, SessionMode, SessionType},
};

/// Props for [`DirCompareView`] — none; state is read from the global atom.
#[allow(dead_code)]
#[derive(Default, Props)]
pub(super) struct DirCompareViewProps;

/// Directory comparison view using ratatui-kit's `Table` component.
///
/// Subscribes to the global [`APP_STATE`] atom for reactive reads, mirrors
/// the session-owned table selection through [`use_session_table_state`],
/// and registers an event handler that delegates to the controller
/// (`dispatch_session_key`); all state mutations go through controller
/// functions. All text entry (filter, save and load names) uses
/// `SearchInput`, which owns its exclusive input layer.
#[component]
pub(super) fn DirCompareView(
    mut hooks: Hooks,
) -> impl Into<AnyElement<'static>> {
    // Subscribe to global state for re-render notifications on writes.
    let app_handle = hooks.use_atom(&APP_STATE);

    // Read session data from the atom.
    let app = app_handle.read();
    let session_selected = app
        .active()
        .filter(|s| s.session_type == SessionType::DirCompare)
        .and_then(|s| s.table_state.selected());

    // Mirror the session-owned selection into the `Table` handle. Called
    // before the early return below so the hook order stays stable.
    let table_state = use_session_table_state(&mut hooks, session_selected);

    let session = match app.active() {
        Some(s) if s.session_type == SessionType::DirCompare => s,
        _ => {
            drop(app);
            return element!(View {});
        }
    };

    // Extract rendering data from the session.
    let sync_active = session.sync_planned.is_some();
    let mode = session.mode.clone();
    let focus = session.focus;
    let hide_same = session.hide_same;
    let active_filter = session.active_filter.clone();
    let left_path_str = session.left_path.to_string_lossy().to_string();
    let right_path_str = session.right_path.to_string_lossy().to_string();
    let sync_operation = session.sync_operation;
    let entries: Vec<DirEntry> =
        session.filtered_entries().into_iter().cloned().collect();
    let sync_items: Vec<TransferItem> = session
        .sync_planned
        .as_ref()
        .map(|s| s.planned.clone())
        .unwrap_or_default();

    // Extract data needed after dropping the app handle (for status bar and
    // element! rendering). All session references below use these locals.
    let filter_input = session.filter_input.clone();
    let sync_planned_count = session
        .sync_planned
        .as_ref()
        .map(|s| s.planned_count())
        .unwrap_or(0);
    let sync_executed = session
        .sync_planned
        .as_ref()
        .map(|s| s.transfer.is_some())
        .unwrap_or(false);
    let comparison_stats = session.comparison.as_ref().map(|c| {
        format!(
            "Total: {}  Same: {}  Diff: {}  Orphans: {}",
            c.total(),
            c.same_count(),
            c.different_count(),
            c.orphan_count(),
        )
    });
    let comparison_is_none = session.comparison.is_none();

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

            if dispatch_session_key(SessionType::DirCompare, key) {
                EventResult::Consumed
            } else {
                EventResult::Ignored
            }
        },
    );

    // -- Build header lines --------------------------------------------------

    let header_lines = [
        Line::from(vec![
            Span::styled("cocomo", Style::new().add_modifier(Modifier::BOLD)),
            Span::raw(" \u{2014} "),
            Span::styled(
                "Folder Compare",
                Style::new().add_modifier(Modifier::DIM),
            ),
        ]),
        Line::from(vec![Span::styled(
            format!("Left:  {left_path_str}"),
            if focus == Focus::Left {
                Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD)
            } else {
                Style::new()
            },
        )]),
        Line::from(vec![Span::styled(
            format!("Right: {right_path_str}"),
            if focus == Focus::Right {
                Style::new().fg(Color::Magenta).add_modifier(Modifier::BOLD)
            } else {
                Style::new()
            },
        )]),
    ];

    // -- Build filter bar line -----------------------------------------------

    let _filter_bar_line: Line<'static> = if hide_same {
        Line::from(Span::styled("No Orphans", Style::new().fg(Color::Yellow)))
    } else {
        Line::from(Span::raw("Show Orphans"))
    };
    let _filter_bar_line = if let Some(filter) = &active_filter {
        Line::from(vec![
            Span::raw("Show Orphans".to_string()),
            Span::raw(" | "),
            Span::styled(format!("*{filter}*"), Style::new().fg(Color::Cyan)),
        ])
    } else {
        _filter_bar_line
    };

    // -- Build status bar line -----------------------------------------------
    // When in Filter, SaveSession, or LoadSession mode, the status bar is
    // replaced by a SearchInput rendered in the element! tree.

    let status_bar_line: Line<'static> = if sync_active {
        let count = sync_planned_count;
        let executed = sync_executed;
        let op_label = sync_operation.label();
        let status_text = if executed {
            "Executed".to_string()
        } else {
            format!("Planned: {} transfers", count)
        };
        Line::from(Span::styled(
            format!("Sync: {op_label}  |  {status_text}"),
            Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD),
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

    // -- Build help bar line -------------------------------------------------
    // Mode-specific hints stay hand-written; the normal-mode bindings come
    // from the keymap so the bar cannot drift from the actual dispatch.

    let help_bar_line: Line<'static> = if mode == SessionMode::Filter {
        Line::from("f=activate  Enter=apply  Esc=cancel")
    } else if matches!(mode, SessionMode::SaveSession { .. }) {
        Line::from("s=activate  Enter=save  Esc=cancel")
    } else if matches!(mode, SessionMode::LoadSession { .. }) {
        Line::from("O=activate  Enter=load  Esc=cancel")
    } else if sync_active {
        Line::from(vec![
            Span::styled("j/k", Style::new().add_modifier(Modifier::BOLD)),
            Span::raw(" navigate  "),
            Span::styled("Esc", Style::new().add_modifier(Modifier::BOLD)),
            Span::raw(" cancel"),
        ])
    } else {
        keys::help_line(&[KeyScope::DirNormal, KeyScope::Global])
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

    let border_style = style::border_style();
    let title_style = style::title_style();
    let comparison_block = Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(" Comparison ", title_style))
        .border_style(border_style);
    let sync_block = Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(" Sync Preview (dry-run) ", title_style))
        .border_style(border_style);

    element!(
        View(flex_direction: Direction::Vertical, height: Constraint::Fill(1)) {
            // Header (3 content lines + 2 border lines = 5).
            Border(
                height: Constraint::Length(5),
                flex_direction: Direction::Vertical,
                border_style: border_style,
            ) {
                Text(text: header_lines[0].clone())
                Text(text: header_lines[1].clone())
                Text(text: header_lines[2].clone())
            }
            // Main content area.
            if sync_active {
                Table<TransferItem>(
                    height: Constraint::Fill(1),
                    state: Some(table_state),
                    active: false,
                    default_index: Some(0),
                    columns: vec![
                        TableColumn::new("Action", Constraint::Length(30)),
                        TableColumn::new("Path", Constraint::Min(20)),
                    ],
                    rows: sync_items,
                    render_row: Some(Arc::new(|item: &TransferItem, _is_selected: bool| {
                        let action_str = item.action.label().to_string();
                        let dir_indicator = if item.is_dir { "\u{1F4C4} " } else { "  " };
                        let action_color = match item.action {
                            TransferAction::CopyLeft | TransferAction::CopyRight => Color::Green,
                            TransferAction::CopyCenter => Color::Yellow,
                            TransferAction::DeleteLeft | TransferAction::DeleteRight => Color::Red,
                            TransferAction::MoveLeft | TransferAction::MoveRight => Color::Magenta,
                        };
                        vec![
                            TableCell::new(action_str)
                                .style(Style::new().fg(action_color).add_modifier(Modifier::BOLD)),
                            TableCell::new(format!("{}{}", dir_indicator, item.rel_path)),
                        ]
                    }) as RenderTableRow<TransferItem>),
                    block: Some(sync_block),
                    highlight_style: style::highlight_style(),
                    highlight_symbol: Some(symbols::block::FULL),
                    column_spacing: 1u16,
                    header_style: style::title_style(),
                )
            } else if entries.is_empty() {
                Border(
                    height: Constraint::Fill(1),
                    border_style: border_style,
                    top_title: Line::from(" Comparison ").fg(Color::Yellow).bold().centered(),
                ) {
                    Text(
                        text: Line::from(empty_msg),
                        alignment: Alignment::Center,
                    )
                }
            } else {
                Table<DirEntry>(
                    height: Constraint::Fill(1),
                    state: Some(table_state),
                    active: false,
                    default_index: Some(0),
                    columns: vec![
                        TableColumn::new("T Name", Constraint::Min(20)),
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
                    rows: entries,
                    render_row: Some(Arc::new(|entry: &DirEntry, _is_selected: bool| {
                        let status_color = status_style(entry.status);

                        let left_meta = entry.left.as_ref().map(|l| {
                            format!(
                                "{:>6} {}",
                                format_size(l.size),
                                format_date(&l.modified)
                            )
                        }).unwrap_or_else(|| "-             ".to_string());

                        let right_meta = entry.right.as_ref().map(|r| {
                            format!(
                                "{:>6} {}",
                                format_size(r.size),
                                format_date(&r.modified)
                            )
                        }).unwrap_or_else(|| "-             ".to_string());

                        let dir_indicator = if entry.left.as_ref().is_some_and(|l| l.is_dir)
                            || entry.right.as_ref().is_some_and(|r| r.is_dir)
                        {
                            "\u{1F5C0}"
                        } else {
                            " "
                        };

                        vec![
                            TableCell::new(format!("{} {}", dir_indicator, entry.name)),
                            TableCell::new(left_meta),
                            TableCell::new(entry.status.symbol())
                                .style(Style::new().fg(status_color).add_modifier(Modifier::BOLD)),
                            TableCell::new(right_meta),
                        ]
                    }) as RenderTableRow<DirEntry>),
                    block: Some(comparison_block),
                    highlight_style: style::highlight_style(),
                    highlight_symbol: Some(symbols::block::FULL),
                    column_spacing: 1u16,
                    header_style: style::title_style(),
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
                Border(height: Constraint::Length(3), border_style: border_style) {
                    Text(text: status_bar_line)
                }
            }
            // Help bar.
            Border(height: Constraint::Length(3), border_style: border_style) {
                Text(text: help_bar_line)
            }
        }
    )
}
