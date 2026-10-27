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
//! case-insensitive regex name filter (with an enable toggle) and the
//! same/different/orphans display checkboxes. `Tab` cycles which section
//! owns the keys; the focused section is marked with a `>` pointer.
//! `Enter` applies the draft to the session and closes the panel; an
//! invalid regex keeps it open and shows the compiler message instead.
//!
//! Like `TransferDialog` the component lives in the App shell and reads
//! the active session, so one implementation serves every dir tab. It
//! owns an exclusive input layer while open and registers its handler in
//! that layer with `EventPriority::High`, so the panel sees `Esc`/`Tab`/
//! `Enter`/`Space` before the child widgets; keys the focused child owns
//! (text editing, the status list's `Space`/`j`/`k`) are reported as
//! unbound and reach the child through the shared layer.

use std::collections::HashSet;

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
    controller::{dispatch_filter_panel_key, set_filter_panel_statuses},
    state::{APP_STATE, FilterDraft, FilterFocus, SessionMode},
};

/// Row labels of the status checkbox list, in display order.
const STATUS_ITEMS: [&str; 3] = ["Same", "Different", "Orphans"];

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
    // checkbox state are minted unconditionally.
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

            // The panel owns `Esc`/`Tab`/`Enter`/`Space` (via the keymap);
            // an unowned key stays unclaimed here so the focused child
            // widget in the same layer still gets it.
            if dispatch_filter_panel_key(key) {
                EventResult::Consumed
            } else {
                EventResult::Ignored
            }
        },
    );

    // The status checkboxes live in the draft as plain bools; the
    // `MultiSelect` wants a `HashSet<usize>`, so mirror the draft into a
    // local state. The effect re-seeds on draft changes with
    // `write_no_update`, so the mirror never feeds a redraw loop back.
    let (seed_same, seed_different, seed_orphans) = draft
        .as_ref()
        .map(|d| (d.show_same, d.show_different, d.show_orphans))
        .unwrap_or((true, true, true));
    let statuses = hooks.use_state(HashSet::<usize>::default);
    hooks.use_effect(
        move || {
            let mut set = statuses.write_no_update();
            set.clear();
            if seed_same {
                set.insert(0);
            }
            if seed_different {
                set.insert(1);
            }
            if seed_orphans {
                set.insert(2);
            }
        },
        (seed_same, seed_different, seed_orphans),
    );

    let Some(draft) = draft else {
        // A childless `Fragment` has a transparent layout and falls back to
        // the default `Fill(1)` height, which would steal the main view's
        // rows in the App shell's vertical flex. A closed `Modal` is
        // zero-sized instead (as in `TransferDialog`).
        return element!(Modal(open: false)).into_any();
    };

    // Flatten the draft into render values.
    let status_items: Vec<String> =
        STATUS_ITEMS.iter().map(|s| s.to_string()).collect();
    let pattern_label = focus_line(
        "Name pattern (regex)",
        draft.focus == FilterFocus::Pattern,
        &theme,
    );
    let input =
        tui_input::Input::new(draft.pattern.clone()).with_cursor(draft.cursor);
    let toggle_line = Line::from(vec![
        focus_marker(draft.focus == FilterFocus::NameToggle, &theme),
        Span::styled(
            if draft.name_filter_on { "[x] " } else { "[ ] " },
            if draft.name_filter_on {
                theme.title
            } else {
                theme.dim
            },
        ),
        Span::raw("name filter enabled"),
    ]);
    let error_line = draft.error.as_ref().map(|e| {
        Line::from(Span::styled(
            e.clone(),
            Style::new()
                .fg(theme.status.different)
                .add_modifier(Modifier::BOLD),
        ))
    });
    // Two border rows plus the padding, the pattern label, the input, the
    // toggle, and the framed status list (three rows plus its border).
    let height = 14 + u16::from(error_line.is_some());

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
                Text(text: toggle_line)
                MultiSelect<String>(
                    items: status_items,
                    selected: statuses,
                    on_change: Handler::from(move |items: Vec<String>| {
                        let has = |label: &str| {
                            items.iter().any(|i| i == label)
                        };
                        set_filter_panel_statuses(
                            has("Same"),
                            has("Different"),
                            has("Orphans"),
                        );
                    }),
                    active: draft.focus == FilterFocus::Statuses,
                    default_index: Some(0),
                    top_title: Some(Line::from(" Statuses ").style(theme.dim)),
                    height: Constraint::Length(5),
                )
                if let Some(line) = error_line {
                    Text(text: line)
                }
            }
        }
    )
    .into_any()
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
