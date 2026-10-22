// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Text comparison (side-by-side diff) view component.

use std::sync::Arc;

use cocomo_lib::Importance;
use ratatui_kit::{
    crossterm::event::{Event, KeyEventKind},
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

use super::{rows::TextDiffRow, style, use_session_table_state};
use crate::{
    controller::dispatch_session_key,
    diff_model::{current_diff_position, diff_line_numbers},
    format::{grammar_style, importance_for_line},
    keys::{self, KeyScope},
    state::{APP_STATE, SessionType},
};

/// Props for [`TextCompareView`] — none; state is read from the global atom.
#[allow(dead_code)]
#[derive(Default, Props)]
pub(super) struct TextCompareViewProps;

/// Text comparison view using ratatui-kit's `Table` component.
///
/// Renders a side-by-side text diff with grammar-based syntax styling and
/// red/green diff highlighting.  Subscribes to the global [`APP_STATE`] atom
/// for reactive reads, mirrors the session-owned table selection through
/// [`use_session_table_state`], and registers an event handler that
/// delegates to the controller (`dispatch_session_key`, which routes
/// text-compare keys to `handle_text_key`).
#[component]
pub(super) fn TextCompareView(
    mut hooks: Hooks,
) -> impl Into<AnyElement<'static>> {
    // Subscribe to global state for re-render notifications on writes.
    let app_handle = hooks.use_atom(&APP_STATE);

    // Read session data from the atom.
    let app = app_handle.read();
    let session_selected = app
        .active()
        .filter(|s| s.session_type == SessionType::TextCompare)
        .and_then(|s| s.table_state.selected());

    // Mirror the session-owned selection into the `Table` handle. Called
    // before the early return below so the hook order stays stable.
    let table_state = use_session_table_state(&mut hooks, session_selected);

    let session = match app.active() {
        Some(s) if s.session_type == SessionType::TextCompare => s,
        _ => {
            drop(app);
            return element!(View {});
        }
    };

    // Extract rendering data from the session.
    let left_path_str = session.left_path.to_string_lossy().to_string();
    let right_path_str = session.right_path.to_string_lossy().to_string();
    let grammar_name = session.grammar.name.clone();
    let left_line_count = session.left_lines.len();
    let right_line_count = session.right_lines.len();

    // Compute diff line numbers and current diff-group position for the
    // status bar.
    let diff_numbers = diff_line_numbers(session);
    let (current_group, total_groups) = current_diff_position(session);

    // Build row data with pre-computed styles (let bindings are not allowed
    // in element! children blocks).
    let max_lines = std::cmp::max(left_line_count, right_line_count);
    let rows: Vec<TextDiffRow> = (0..max_lines)
        .map(|i| {
            let left_line = session.left_lines.get(i);
            let right_line = session.right_lines.get(i);

            // Left side formatting.
            let left_num = left_line.map(|l| l.number).unwrap_or(0);
            let left_content = left_line
                .as_ref()
                .map(|l| l.content.clone())
                .unwrap_or_default();
            let left_is_diff =
                left_line.is_some_and(|l| diff_numbers.contains(&l.number));
            let left_importance = left_line
                .map(importance_for_line)
                .unwrap_or(Importance::Ignored);
            let left_style = if left_is_diff {
                Style::new().fg(Color::Red).add_modifier(Modifier::BOLD)
            } else {
                grammar_style(left_importance)
            };

            // Right side formatting.
            let right_num = right_line.map(|l| l.number).unwrap_or(0);
            let right_content = right_line
                .as_ref()
                .map(|l| l.content.clone())
                .unwrap_or_default();
            let right_is_diff =
                right_line.is_some_and(|l| diff_numbers.contains(&l.number));
            let right_importance = right_line
                .map(importance_for_line)
                .unwrap_or(Importance::Ignored);
            let right_style = if right_is_diff {
                Style::new().fg(Color::Green).add_modifier(Modifier::BOLD)
            } else {
                grammar_style(right_importance)
            };

            TextDiffRow {
                left_num: format!("{left_num:<6}"),
                left_marker: if left_is_diff { "\u{25B6} " } else { "  " },
                left_content,
                left_style,
                right_num: format!("{right_num:<6}"),
                right_marker: if right_is_diff { "\u{25B6} " } else { "  " },
                right_content,
                right_style,
            }
        })
        .collect();

    // Build header line.
    let header_line = Line::from(format!(
            "Text Diff  |  {}: {left_line_count}L  |  {}: \
             {right_line_count}L  |  Grammar: {}",
            left_path_str, right_path_str, grammar_name
        ));

    // Build status bar line.
    let position_str = if total_groups > 0 {
        if current_group > 0 {
            format!("Diff {current_group}/{total_groups}")
        } else {
            format!("Diff 0/{total_groups} (no diff selected)")
        }
    } else {
        "No differences".to_string()
    };
    let status_bar_line = Line::from(format!(
        "{position_str}  |  Left: {left_line_count} lines  |  Right: \
         {right_line_count} lines"
    ));

    // Build help bar line from the keymap so it cannot drift from the
    // actual bindings.
    let help_bar_line: Line<'static> =
        keys::help_line(&[KeyScope::TextNormal, KeyScope::Global]);

    // File names for table column headers.
    let left_name = session
        .left_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let right_name = session
        .right_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();

    drop(app);

    // Event handler for session-level keys (j/k, g/G, n/N, l/Tab).
    // The App component's global handler runs first and consumes global keys
    // (Ctrl+W, Ctrl+Tab, Left/Right, q).  For remaining keys, the controller
    // locks the state briefly, lets `handle_session_key` (which routes
    // text-compare keys to `handle_text_key`) mutate the session, and
    // dispatches the resulting intent.
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

            if dispatch_session_key(SessionType::TextCompare, key) {
                EventResult::Consumed
            } else {
                EventResult::Ignored
            }
        },
    );

    // -- Build the element tree ----------------------------------------------

    let border_style = style::border_style();
    let title_style = style::title_style();
    let text_block = Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(" Text Diff ", title_style))
        .border_style(border_style);

    element!(
        View(flex_direction: Direction::Vertical, height: Constraint::Fill(1)) {
            // Header (1 content line + 2 border lines = 3).
            Border(
                height: Constraint::Length(3),
                border_style: border_style,
            ) {
                Text(text: header_line)
            }
            // Main content area.
            if rows.is_empty() {
                Border(
                    height: Constraint::Fill(1),
                    border_style: border_style,
                    top_title: Line::from(" Text Diff ").fg(Color::Yellow).bold().centered(),
                ) {
                    Text(
                        text: Line::from("Empty file(s)."),
                        alignment: Alignment::Center,
                    )
                }
            } else {
                Table<TextDiffRow>(
                    height: Constraint::Fill(1),
                    state: Some(table_state),
                    active: false,
                    default_index: Some(0),
                    columns: vec![
                        TableColumn::new("Num", Constraint::Length(7)),
                        TableColumn::new("", Constraint::Length(2)),
                        TableColumn::new(
                            Line::from(vec![Span::styled(
                                left_name.clone(),
                                Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
                            )]),
                            Constraint::Min(20),
                        ),
                        TableColumn::new("Num", Constraint::Length(7)),
                        TableColumn::new("", Constraint::Length(2)),
                        TableColumn::new(
                            Line::from(vec![Span::styled(
                                right_name.clone(),
                                Style::new().fg(Color::Magenta).add_modifier(Modifier::BOLD),
                            )]),
                            Constraint::Min(20),
                        ),
                    ],
                    rows: rows,
                    render_row: Some(Arc::new(|row: &TextDiffRow, _is_selected: bool| {
                        vec![
                            TableCell::new(row.left_num.clone())
                                .style(Style::new().fg(Color::DarkGray)),
                            TableCell::new(row.left_marker).style(row.left_style),
                            TableCell::new(row.left_content.clone()).style(row.left_style),
                            TableCell::new(row.right_num.clone())
                                .style(Style::new().fg(Color::DarkGray)),
                            TableCell::new(row.right_marker).style(row.right_style),
                            TableCell::new(row.right_content.clone()).style(row.right_style),
                        ]
                    }) as RenderTableRow<TextDiffRow>),
                    block: Some(text_block),
                    highlight_style: style::highlight_style(),
                    highlight_symbol: Some(symbols::block::FULL),
                    column_spacing: 1u16,
                    header_style: style::title_style(),
                )
            }
            // Status bar.
            Border(height: Constraint::Length(3), border_style: border_style) {
                Text(text: status_bar_line)
            }
            // Help bar.
            Border(height: Constraint::Length(3), border_style: border_style) {
                Text(text: help_bar_line)
            }
        }
    )
}
