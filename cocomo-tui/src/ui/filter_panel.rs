// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Filter panel modal: the `f` entry point for all view filters.
//!
//! The panel edits a [`FilterDraft`] held by the active session: a
//! case-insensitive regex name pattern and the "Show items:" checkbox
//! list (hidden, same/equal, different with its newer-side subs, orphan
//! with its side subs). `Tab` cycles which section owns the keys; while
//! the pattern has focus its label carries the `>` pointer, while the
//! checkbox list has focus the pointer marks the selected row. `Space`
//! toggles that checkbox, `j`/`k` move the row cursor (the panel's key
//! handling lives in [`crate::keys`]). `Enter` applies the draft to the
//! session and closes the panel; an invalid regex keeps it open and shows
//! the compiler message instead.
//!
//! Like `TransferDialog` the component lives in the App shell and reads
//! the active session, so one implementation serves every dir tab. It
//! owns an exclusive input layer while open and registers its handler in
//! that layer with `EventPriority::High`, so the panel sees its keys
//! before the child widgets; the only child that needs keys is the
//! pattern `Input` (rendering the buffer and cursor), and unowned keys
//! are reported as unclaimed so it still gets them through the shared
//! layer.

use ratatui_kit::{
    crossterm::event::{Event, KeyEventKind},
    prelude::*,
    ratatui::{
        layout::{Constraint, Direction},
        style::{Modifier, Style},
        text::{Line, Span},
        widgets::Padding,
    },
};

use super::{frame::hint_line, theme::CocomoTheme};
use crate::{
    controller::dispatch_filter_panel_key,
    state::{APP_STATE, FilterDraft, FilterFocus, FilterRow, SessionMode},
};

/// Filter panel modal rendered above the active session.
#[component]
pub(super) fn FilterPanel(mut hooks: Hooks) -> impl Into<AnyElement<'static>> {
    let app_handle = hooks.use_atom(&APP_STATE);
    let theme = hooks.use_component_theme::<CocomoTheme>();

    // Resolve the draft from the active session; cloning it keeps the read
    // guard from living into the render below.
    let draft: Option<FilterDraft> =
        app_handle.read().active().and_then(|s| match &s.ui.mode {
            SessionMode::FilterPanel { draft } => Some(*draft.clone()),
            _ => None,
        });
    let open = draft.is_some();

    // All hooks must run on every frame, open or not, so the layer and the
    // handler are minted unconditionally.
    let layer = hooks.use_input_layer(open, true);
    hooks.use_event_handler(
        EventScope::Layer(layer),
        EventPriority::High,
        move |event| {
            let Event::Key(key) = event else {
                return EventResult::Ignored;
            };
            if key.kind != KeyEventKind::Press {
                return EventResult::Ignored;
            }

            // The panel owns its keys (via the keymap and the focused
            // section); an unowned key stays unclaimed here so the child
            // widget in the same layer still gets it.
            if dispatch_filter_panel_key(key) {
                EventResult::Consumed
            } else {
                EventResult::Ignored
            }
        },
    );

    let Some(draft) = draft else {
        // A childless `Fragment` has a transparent layout and falls back to
        // the default `Fill(1)` height, which would steal the main view's
        // rows in the App shell's vertical flex. A closed `Modal` is
        // zero-sized instead (as in `TransferDialog`).
        return element!(Modal(open: false)).into_any();
    };

    // Flatten the draft into render values.
    let pattern_label = focus_line(
        "Name pattern:",
        draft.focus == FilterFocus::Pattern,
        &theme,
    );
    let input =
        tui_input::Input::new(draft.pattern.clone()).with_cursor(draft.cursor);
    let rows =
        checkbox_rows(&draft, draft.focus == FilterFocus::Statuses, &theme);
    let error_line = draft.error.as_ref().map(|e| {
        Line::from(Span::styled(
            e.clone(),
            Style::new()
                .fg(theme.status.different)
                .add_modifier(Modifier::BOLD),
        ))
    });
    // Two border rows plus the padding, the pattern label, the input, a
    // blank, the section header, a blank, and the nine checkbox rows
    // (eight checkboxes and the separator after the hidden row).
    let height = 18 + u16::from(error_line.is_some());

    element!(
        Modal(
            open: true,
            // Reuse this component's layer instead of letting `Modal` mint
            // its own, which would deafen the handler above.
            layer: Some(layer),
            width: Constraint::Length(60),
            height: Constraint::Length(height),
        ) {
            Border(
                flex_direction: Direction::Vertical,
                border_style: theme.border,
                top_title: Line::from(" Filter ")
                    .style(theme.title)
                    .centered(),
                bottom_title: hint_line(&[
                    ("Tab", "focus"),
                    ("Space", "toggle"),
                    ("Enter", "apply"),
                    ("Esc", "cancel"),
                ])
                .style(theme.dim)
                .centered(),
                padding: Padding::uniform(1),
            ) {
                Text(text: pattern_label)
                Input(
                    input: input,
                    placeholder: "regex, e.g. \\.rs$".to_string(),
                    hide_cursor: draft.focus != FilterFocus::Pattern,
                )
                Text(text: Line::from(""))
                Text(text: Line::from(Span::styled("Show items:", theme.dim)))
                Text(text: Line::from(""))
                for (index, line) in rows.into_iter().enumerate() {
                    Text(key: index, text: line)
                }
                if let Some(line) = error_line {
                    Text(text: line)
                }
            }
        }
    )
    .into_any()
}

/// The "Show items:" checkbox rows in display order.
///
/// Blank separator rows separate the hidden toggle from the status groups.
/// Checkbox rows lead with the focus pointer (only while the section has
/// focus and the row is selected), sub-checkboxes are indented under their
/// umbrella, and the `[x]`/`[ ]` box reads the draft directly — the draft
/// is the single source of truth for the check state.
fn checkbox_rows(
    draft: &FilterDraft,
    active: bool,
    theme: &CocomoTheme,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for row in FilterRow::ALL {
        lines.push(checkbox_line(row, draft, active, theme));
        if row == FilterRow::Hidden {
            // Separator between the hidden group and the status groups.
            lines.push(Line::from(""));
        }
    }
    lines
}

/// Render one checkbox line: pointer, indentation, box and label.
fn checkbox_line(
    row: FilterRow,
    draft: &FilterDraft,
    active: bool,
    theme: &CocomoTheme,
) -> Line<'static> {
    let selected = active && FilterRow::ALL[draft.selected] == row;
    let indent = if row.is_child() { "    " } else { "" };
    let checked = draft.row_shown(row);
    Line::from(vec![
        focus_marker(selected, theme),
        Span::raw(indent),
        Span::styled(
            if checked { "[x] " } else { "[ ] " },
            if checked { theme.title } else { theme.dim },
        ),
        Span::raw(row.label()),
    ])
}

/// A section label with a `>` pointer while the section has focus.
fn focus_line(
    label: &'static str,
    focused: bool,
    theme: &CocomoTheme,
) -> Line<'static> {
    Line::from(vec![focus_marker(focused, theme), Span::raw(label)])
}

/// The focus pointer span: a bold `>` on focused sections, a blank pad
/// otherwise so the labels stay aligned.
fn focus_marker(focused: bool, theme: &CocomoTheme) -> Span<'static> {
    if focused {
        Span::styled("> ", theme.title)
    } else {
        Span::styled("  ", theme.dim)
    }
}

#[cfg(test)]
mod tests {
    use cocomo_lib::{DirEntryStatus, Endpoint};
    use ratatui_kit::{ratatui::buffer::Buffer, test_util::render_frames};

    use super::*;
    use crate::{
        state::{AppState, SessionView},
        ui::theme::cocomo_palette,
    };

    /// A dir session whose only content is the given open filter panel.
    fn app_with_draft(draft: FilterDraft) -> AppState {
        let mut session = SessionView::new_dir_compare(
            Endpoint::local("/left"),
            Endpoint::local("/right"),
            false,
        );
        session.ui.mode = SessionMode::FilterPanel {
            draft: Box::new(draft),
        };
        let mut app = AppState::new();
        app.add_session(session);
        app
    }

    /// Render the open panel into a buffer sized to its fixed layout.
    fn render(draft: FilterDraft) -> Buffer {
        APP_STATE.set(app_with_draft(draft));
        render_frames(
            element!(PaletteProvider(palette: cocomo_palette()) {
                FilterPanel()
            }),
            60,
            20,
            1,
        )
    }

    /// First content column inside the panel (border plus padding).
    const X: u16 = 2;
    /// First content row inside the panel (border plus padding).
    const Y0: u16 = 3;

    #[test]
    fn checkbox_rows_show_the_draft_state_and_cursor() {
        let draft = FilterDraft {
            focus: FilterFocus::Statuses,
            show_hidden: false,
            statuses: DirEntryStatus::all() - DirEntryStatus::EQUAL,
            selected: 2,
            ..FilterDraft::default()
        };
        let buf = render(draft);

        // Pattern label, input, blank, section header, blank, then the
        // hidden row; `selected` 2 addresses the "different" row.
        let hidden_y = Y0 + 5;
        let different_y = hidden_y + 3;
        assert_eq!(buf[(X, different_y)].symbol(), ">");
        // Unselected rows pad instead.
        assert_eq!(buf[(X, hidden_y)].symbol(), " ");
        // The hidden checkbox is unchecked, the different one checked.
        assert_eq!(buf[(X + 2, hidden_y)].symbol(), "[");
        assert_eq!(buf[(X + 3, hidden_y)].symbol(), " ");
        assert_eq!(buf[(X + 3, different_y)].symbol(), "x");
        // A blank separator row sits between hidden and the status group.
        assert_eq!(buf[(X + 2, hidden_y + 1)].symbol(), " ");
        // The sub-checkboxes are indented under their umbrellas.
        let left_newer_y = hidden_y + 4;
        assert_eq!(buf[(X + 6, left_newer_y)].symbol(), "[");
        assert_eq!(buf[(X + 7, left_newer_y)].symbol(), "x");
    }

    #[test]
    fn pattern_focus_hides_the_row_pointer() {
        let draft = FilterDraft {
            focus: FilterFocus::Pattern,
            ..FilterDraft::default()
        };
        let buf = render(draft);
        // The pattern label carries the pointer instead of any row.
        assert_eq!(buf[(X, Y0)].symbol(), ">");
        let hidden_y = Y0 + 5;
        assert_eq!(buf[(X, hidden_y)].symbol(), " ");
        assert_eq!(buf[(X, hidden_y + 3)].symbol(), " ");
    }
}
