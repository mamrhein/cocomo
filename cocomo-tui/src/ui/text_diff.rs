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

use ratatui_kit::{
    crossterm::event::{Event, KeyCode, KeyEventKind},
    prelude::*,
    ratatui::{
        layout::{Constraint, Direction},
        text::{Line, Span},
    },
};

use super::{
    diff_list::SessionDiffList,
    frame::{COMMAND_PLACEHOLDER, HelpBar, StatusBar},
    rows::{TextDiffRow, text_diff_rows},
    theme::CocomoTheme,
};
use crate::{
    controller::{
        dispatch_text_key, set_active_mode, set_mode_buffer, submit_command,
        submit_search,
    },
    diff_model::{current_diff_position, diff_line_numbers_by_side},
    keys::{self, KeyScope},
    state::{APP_STATE, SessionMode},
};

/// Text comparison view using a windowed side-by-side diff list.
///
/// Renders the diff with grammar-based syntax styling and red/green diff
/// highlighting.  Subscribes to the global [`APP_STATE`] atom for reactive
/// reads, registers an event handler that delegates to the controller
/// (`dispatch_text_key`, which routes text-compare keys to
/// `handle_session_key`), and memoizes the diff rows: they are rebuilt
/// only when the session or its line counts change, and the list renders
/// just the visible window.
#[component]
pub(super) fn TextCompareView(
    mut hooks: Hooks,
) -> impl Into<AnyElement<'static>> {
    // Subscribe to global state for re-render notifications on writes.
    let app_handle = hooks.use_atom(&APP_STATE);
    let theme = hooks.use_component_theme::<CocomoTheme>();
    // Terminal width drives the help bar wrapping; the hook re-renders on
    // resize. Must stay above the early returns below (stable hook order).
    let (term_width, _) = hooks.use_terminal_size();

    // Event handler for session-level keys (j/k, g/G, n/N, l/Tab).
    // The App component's global handler runs first and consumes global keys
    // (Ctrl+W, Ctrl+Tab, Left/Right, q).  For remaining keys, the controller
    // locks the state briefly, lets `handle_session_key` mutate the session,
    // and dispatches the resulting intent. Like the hooks above it, this
    // registration must stay above the early returns below (stable hook
    // order); it captures nothing session-specific, so it can run before the
    // session is resolved.
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

            if dispatch_text_key(key) {
                EventResult::Consumed
            } else {
                EventResult::Ignored
            }
        },
    );

    // Read session data from the atom.
    let app = app_handle.read();
    let session = app.active().filter(|s| s.content.is_text());
    let session_selected = session.and_then(|s| s.ui.table_state.selected());

    // Memo key: the session identity plus the line counts. The text
    // content is immutable once the session is created, so the counts
    // fully determine the rows.
    let rows_key = session.and_then(|s| {
        let text = s.text()?;
        Some((s.core.id, text.left_lines.len(), text.right_lines.len()))
    });
    let rows: Arc<Vec<TextDiffRow>> = hooks.use_memo(
        || match session {
            Some(s) => match s.text() {
                Some(text) => {
                    let (left_diffs, right_diffs) =
                        diff_line_numbers_by_side(s);
                    text_diff_rows(
                        &text.left_lines,
                        &text.right_lines,
                        &left_diffs,
                        &right_diffs,
                        &theme,
                    )
                }
                None => Arc::new(Vec::new()),
            },
            None => Arc::new(Vec::new()),
        },
        rows_key,
    );

    let Some(session) = session else {
        drop(app);
        return element!(View {});
    };
    // The filter above guarantees a text session; the binding keeps the
    // borrow checker happy without a panic path.
    let Some(text) = session.text() else {
        drop(app);
        return element!(View {});
    };

    // Extract rendering data from the session.
    let left_path_str = session.core.left.display().to_string();
    let right_path_str = session.core.right.display().to_string();
    let grammar_name = text.grammar.name.clone();
    let left_line_count = text.left_lines.len();
    let right_line_count = text.right_lines.len();
    let show_help = app.show_help;
    let mode = session.ui.mode.clone();

    // Compute the current diff-group position for the status bar. The row
    // styles already carry the per-line diff highlighting from the memo.
    let (current_group, total_groups) = current_diff_position(session);

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
    let mut status_bar_line = Line::from(format!(
        "{position_str}  |  Left: {left_line_count} lines  |  Right: \
         {right_line_count} lines"
    ));

    // When the help bar is hidden, hint at the toggle key so the binding
    // stays discoverable without spending a row on it.
    if !show_help {
        status_bar_line
            .spans
            .push(Span::styled("  ? help", theme.dim));
    }

    // Build help bar lines from the keymap, wrapped to the terminal width,
    // so the bar cannot drift from the actual bindings and never truncates
    // an item. While a text-entry mode is active, mode-specific hints take
    // over (the same convention as the dir view).
    let help_bar_lines: Vec<Line<'static>> =
        if matches!(mode, SessionMode::Search { .. }) {
            vec![Line::from("/=activate  Enter=jump  Esc=cancel")]
        } else if matches!(mode, SessionMode::Command { .. }) {
            vec![Line::from(":=activate  Enter=run  Esc=cancel")]
        } else {
            keys::help_lines(
                &[KeyScope::TextNormal, KeyScope::Global],
                term_width,
            )
        };

    // File names for the diff column headers.
    let left_name = session
        .core
        .left
        .path()
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let right_name = session
        .core
        .right
        .path()
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();

    drop(app);

    // -- Build the element tree ----------------------------------------------

    element!(
        View(flex_direction: Direction::Vertical, height: Constraint::Fill(1)) {
            // Header: a single borderless line (the diff panel below draws
            // the separating border).
            View(height: Constraint::Length(1)) {
                Text(text: header_line)
            }
            // Main content area: windowed side-by-side diff.
            SessionDiffList(
                selected: session_selected,
                title: "Text Diff",
                empty_message: "Empty file(s).",
                rows: rows,
                left_name: left_name,
                right_name: right_name,
            )
            // Status bar or input component depending on mode. Text entry
            // uses SearchInput, which owns its exclusive input layer; the
            // activate key matches the key that entered the mode.
            if let SessionMode::Search { query } = &mode {
                SearchInput(
                    value: query.clone(),
                    placeholder: "Search lines...".to_string(),
                    is_editing: true,
                    activate_key: KeyCode::Char('/'),
                    on_change: Handler::from(move |val: String| {
                        set_mode_buffer(val);
                    }),
                    on_submit: Handler::from(move |val: String| -> bool {
                        submit_search(val)
                    }),
                    clear_on_escape: true,
                    on_clear: Handler::from(move |_: ()| {
                        set_active_mode(SessionMode::Normal);
                    }),
                )
            } else if let SessionMode::Command { input } = &mode {
                SearchInput(
                    value: input.clone(),
                    placeholder: COMMAND_PLACEHOLDER.to_string(),
                    is_editing: true,
                    activate_key: KeyCode::Char(':'),
                    on_change: Handler::from(move |val: String| {
                        set_mode_buffer(val);
                    }),
                    on_submit: Handler::from(move |val: String| -> bool {
                        submit_command(val)
                    }),
                    clear_on_escape: true,
                    on_clear: Handler::from(move |_: ()| {
                        set_active_mode(SessionMode::Normal);
                    }),
                )
            } else {
                StatusBar(line: status_bar_line)
            }
            // Help bar: only rendered when toggled on (`?`).
            if show_help {
                HelpBar(lines: help_bar_lines)
            }
        }
    )
}
