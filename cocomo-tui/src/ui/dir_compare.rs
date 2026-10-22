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
// `tui_input::Input` is the underlying input-state type used by the Input
// component and for manual save/load session name entry.
use ratatui_kit::components::tui_input;
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
// The `EventHandler` trait provides `handle_event` on `tui_input::Input`
// for processing crossterm key events (character insertion, backspace,
// etc.).
use tui_input::backend::crossterm::EventHandler as _;

use super::style;
use crate::{
    controller::{
        dispatch_session_key, process_session_action, set_active_mode,
        set_filter_input, submit_filter,
    },
    format::{format_date, format_size, status_style},
    session_store::default_session_dir,
    state::{APP_STATE, Focus, SessionAction, SessionMode, SessionType},
};

/// Props for [`DirCompareView`] — none; state is read from the global atom.
#[allow(dead_code)]
#[derive(Default, Props)]
pub(super) struct DirCompareViewProps;

/// Directory comparison view using ratatui-kit's `Table` component.
///
/// Subscribes to the global [`APP_STATE`] atom for reactive reads, uses a
/// local `State<TableState>` for table selection that is synced from the
/// session on each render, and registers an event handler that delegates to
/// the controller (`dispatch_session_key`); all state mutations go through
/// controller functions.
#[component]
pub(super) fn DirCompareView(
    mut hooks: Hooks,
) -> impl Into<AnyElement<'static>> {
    // Subscribe to global state for re-render notifications on writes.
    let app_handle = hooks.use_atom(&APP_STATE);

    // Local table selection state, synced from the session's table_state.
    let table_state = hooks.use_state(TableState::default);

    // Read session data from the atom.
    let app = app_handle.read();
    let active_tab = app.active_tab;
    let session = match app.active() {
        Some(s) if s.session_type == SessionType::DirCompare => s,
        _ => {
            drop(app);
            return element!(View {});
        }
    };

    // Sync local table state from the session's table_state (no re-render
    // trigger).
    let session_selected = session.table_state.selected();
    if table_state.read().selected() != session_selected {
        table_state.write_no_update().select(session_selected);
    }

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

    // Tab-switch sync: when the active tab changes, re-sync local table state
    // from the new session's table_state.
    let ts_for_effect = table_state;
    hooks.use_effect(
        move || {
            let app_read = app_handle.read();
            if let Some(s) = app_read.active()
                && s.session_type == SessionType::DirCompare
            {
                let selected = s.table_state.selected();
                ts_for_effect.write_no_update().select(selected);
            }
        },
        active_tab,
    );

    // Local state for the save/load session name input (tui_input::Input).
    // Synced from session.filter_input when entering SaveSession/LoadSession
    // mode.
    let save_input = hooks.use_state(tui_input::Input::default);

    // Sync save_input from session.filter_input when in
    // SaveSession/LoadSession mode.
    if (mode == SessionMode::SaveSession || mode == SessionMode::LoadSession)
        && save_input.read().value() != filter_input.as_str()
    {
        *save_input.write() = tui_input::Input::new(filter_input.clone());
    }

    // Input layer for save/load session name entry.  Active only when the
    // session is in SaveSession or LoadSession mode.
    let input_layer = hooks.use_input_layer(
        mode == SessionMode::SaveSession || mode == SessionMode::LoadSession,
        true,
    );

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

    // Custom input layer handler for save/load session name entry.
    // Uses the Input component with manual tui_input::Input state management.
    let si = save_input;
    hooks.use_event_handler(
        EventScope::Layer(input_layer),
        EventPriority::High,
        move |event| {
            let Event::Key(key) = event else {
                return EventResult::Ignored;
            };
            if key.kind != KeyEventKind::Press {
                return EventResult::Ignored;
            }

            // Only active when in SaveSession/LoadSession mode.
            let current_mode = app_handle
                .read()
                .active()
                .map(|s| s.mode.clone())
                .unwrap_or(SessionMode::Normal);
            if current_mode != SessionMode::SaveSession
                && current_mode != SessionMode::LoadSession
            {
                return EventResult::Ignored;
            }

            match key.code {
                KeyCode::Char(_) | KeyCode::Backspace => {
                    si.write().handle_event(&Event::Key(key));
                    set_filter_input(si.read().value().to_string());
                    EventResult::Consumed
                }
                KeyCode::Enter => {
                    let name = si.read().value().to_string();
                    set_active_mode(SessionMode::Normal);

                    match current_mode {
                        SessionMode::SaveSession => {
                            let save_name = if name.is_empty() {
                                app_handle
                                    .read()
                                    .active()
                                    .map(|s| s.name.clone())
                                    .unwrap_or_default()
                            } else {
                                name
                            };
                            process_session_action(
                                SessionAction::SaveSession { name: save_name },
                            );
                        }
                        SessionMode::LoadSession => {
                            let session_dir = default_session_dir();
                            let path =
                                session_dir.join(format!("{}.toml", name));
                            process_session_action(
                                SessionAction::LoadSession { path },
                            );
                        }
                        _ => {}
                    }
                    EventResult::Consumed
                }
                KeyCode::Esc => {
                    set_active_mode(SessionMode::Normal);
                    EventResult::Consumed
                }
                _ => EventResult::Ignored,
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
    // replaced by SearchInput or Input components rendered in the element!
    // tree.

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

    let help_bar_line: Line<'static> = if mode == SessionMode::Filter {
        Line::from("f=activate  Enter=apply  Esc=cancel")
    } else if mode == SessionMode::SaveSession {
        Line::from("Enter=save  Esc=cancel")
    } else if mode == SessionMode::LoadSession {
        Line::from("Enter=load  Esc=cancel")
    } else if sync_active {
        Line::from(vec![
            Span::styled("j/k", Style::new().add_modifier(Modifier::BOLD)),
            Span::raw(" navigate  "),
            Span::styled("Esc", Style::new().add_modifier(Modifier::BOLD)),
            Span::raw(" cancel"),
        ])
    } else {
        Line::from(vec![
            Span::styled("j/k", Style::new().add_modifier(Modifier::BOLD)),
            Span::raw(" navigate  "),
            Span::styled("l/Tab", Style::new().add_modifier(Modifier::BOLD)),
            Span::raw(" focus  "),
            Span::styled("f", Style::new().add_modifier(Modifier::BOLD)),
            Span::raw(" filter  "),
            Span::styled("F", Style::new().add_modifier(Modifier::BOLD)),
            Span::raw(" hide same  "),
            Span::styled("C", Style::new().add_modifier(Modifier::BOLD)),
            Span::raw(" cont. on/off  "),
            Span::styled("m/M", Style::new().add_modifier(Modifier::BOLD)),
            Span::raw(" sync op  "),
            Span::styled("P", Style::new().add_modifier(Modifier::BOLD)),
            Span::raw(" plan  "),
            Span::styled("e", Style::new().add_modifier(Modifier::BOLD)),
            Span::raw(" execute  "),
            Span::styled("r", Style::new().add_modifier(Modifier::BOLD)),
            Span::raw(" reload  "),
            Span::styled("s", Style::new().add_modifier(Modifier::BOLD)),
            Span::raw(" save  "),
            Span::styled("O", Style::new().add_modifier(Modifier::BOLD)),
            Span::raw(" load  "),
            Span::styled(
                "\u{21A5}",
                Style::new().add_modifier(Modifier::BOLD),
            ),
            Span::raw(" open dir  "),
            Span::styled(
                "\u{232B}",
                Style::new().add_modifier(Modifier::BOLD),
            ),
            Span::raw(" up  "),
            Span::styled("Ctrl+W", Style::new().add_modifier(Modifier::BOLD)),
            Span::raw(" close  "),
            Span::styled(
                "Ctrl+Tab",
                Style::new().add_modifier(Modifier::BOLD),
            ),
            Span::raw(" tabs  "),
            Span::styled("q", Style::new().add_modifier(Modifier::BOLD)),
            Span::raw(" quit"),
        ])
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
            // Status bar or input component depending on mode.
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
            } else if mode == SessionMode::SaveSession
                || mode == SessionMode::LoadSession
            {
                Border(height: Constraint::Length(3), border_style: border_style) {
                    Input(
                        input: save_input.read().clone(),
                        placeholder: "Session name...".to_string(),
                        hide_cursor: false,
                    )
                }
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
