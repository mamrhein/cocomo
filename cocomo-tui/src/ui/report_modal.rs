// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Report modal: the `R` entry point for comparison reports.
//!
//! The modal edits a [`ReportDraft`] held by the active session: the
//! output format (text, CSV, or JSON), the destination file path, and the
//! inclusion checkboxes that become the [`cocomo_lib::report::ReportConfig`].
//! `Tab` cycles which section owns the keys; `Enter` on the format list
//! confirms the highlighted format, while `Enter` anywhere else generates
//! the report and writes it to the destination. An empty destination keeps
//! the modal open with a validation message; an I/O failure surfaces in
//! the `AlertModal` while the draft stays editable.
//!
//! Like `TransferDialog` the component lives in the App shell and reads
//! the active session. Its handler runs in the modal's exclusive input
//! layer with `EventPriority::High`; it owns `Esc`/`Tab` and `Enter`
//! outside the format list, and reports everything else as unbound so the
//! focused child widget still gets it.

use std::collections::HashSet;

use cocomo_lib::report::ReportFormat;
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

use super::theme::CocomoTheme;
use crate::{
    controller::{dispatch_report_key, set_report_format, set_report_options},
    state::{APP_STATE, ReportDraft, ReportFocus, SessionMode},
};

/// Row labels of the format list, in display order.
const FORMAT_ITEMS: [&str; 3] = ["Text", "Csv", "Json"];

/// Row labels of the inclusion checkbox list, in display order.
const OPTION_ITEMS: [&str; 5] = [
    "Same entries",
    "Different entries",
    "Orphan entries",
    "Subdirectories",
    "File details",
];

/// Report modal rendered above the active session.
#[component]
pub(super) fn ReportModal(mut hooks: Hooks) -> impl Into<AnyElement<'static>> {
    let app_handle = hooks.use_atom(&APP_STATE);
    let theme = hooks.use_component_theme::<CocomoTheme>();

    // Resolve the draft from the active session; cloning it keeps the read
    // guard from living into the render below.
    let draft: Option<ReportDraft> =
        app_handle.read().active().and_then(|s| match &s.ui.mode {
            SessionMode::Report { draft } => Some(*draft.clone()),
            _ => None,
        });
    let open = draft.is_some();

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

            // The modal owns `Esc`/`Tab` and `Enter` outside the format
            // list (via the keymap and the dynamic rule in `keys`); an
            // unowned key stays unclaimed here so the focused child widget
            // in the same layer still gets it.
            if dispatch_report_key(key) {
                EventResult::Consumed
            } else {
                EventResult::Ignored
            }
        },
    );

    // The inclusion checkboxes live in the draft as plain bools; mirror
    // them into the `HashSet<usize>` the `MultiSelect` wants. The effect
    // re-seeds on draft changes with `write_no_update`, so the mirror
    // never feeds a redraw loop back.
    let seeds: [bool; 5] =
        draft.as_ref().map_or([false, true, true, true, true], |d| {
            [
                d.include_same,
                d.include_different,
                d.include_orphans,
                d.include_subdirectories,
                d.include_file_details,
            ]
        });
    let options = hooks.use_state(HashSet::<usize>::default);
    hooks.use_effect(
        move || {
            let mut set = options.write_no_update();
            set.clear();
            for (index, on) in seeds.into_iter().enumerate() {
                if on {
                    set.insert(index);
                }
            }
        },
        seeds,
    );

    let Some(draft) = draft else {
        // A childless `Fragment` has a transparent layout and falls back to
        // the default `Fill(1)` height, which would steal the main view's
        // rows in the App shell's vertical flex. A closed `Modal` is
        // zero-sized instead (as in `TransferDialog`).
        return element!(Modal(open: false)).into_any();
    };

    // Flatten the draft into render values.
    let format_items: Vec<String> =
        FORMAT_ITEMS.iter().map(|s| s.to_string()).collect();
    let option_items: Vec<String> =
        OPTION_ITEMS.iter().map(|s| s.to_string()).collect();
    let format_label = focus_line(
        "Format (Enter confirms)",
        draft.focus == ReportFocus::Format,
        &theme,
    );
    let destination_label = focus_line(
        "Destination",
        draft.focus == ReportFocus::Destination,
        &theme,
    );
    let input = tui_input::Input::new(draft.destination.clone())
        .with_cursor(draft.cursor);
    let format_index = match draft.format {
        ReportFormat::Text => 0,
        ReportFormat::Csv => 1,
        ReportFormat::Json => 2,
    };
    let error_line = draft.error.as_ref().map(|e| {
        Line::from(Span::styled(
            e.clone(),
            Style::new()
                .fg(theme.status.different)
                .add_modifier(Modifier::BOLD),
        ))
    });
    // Two border rows plus the padding, the two labels, the input, the
    // framed format list (three rows plus its border), and the framed
    // option list (five rows plus its border).
    let height = 19 + u16::from(error_line.is_some());

    element!(
        Modal(
            open: true,
            // Reuse this component's layer; see `FilterPanel`.
            layer: Some(layer),
            width: Constraint::Length(60),
            height: Constraint::Length(height),
        ) {
            Border(
                flex_direction: Direction::Vertical,
                border_style: theme.border,
                top_title: Line::from(" Report ")
                    .style(theme.title)
                    .centered(),
                bottom_title: Line::from(
                    " Tab focus · Enter generate · Esc close ",
                )
                .style(theme.dim)
                .centered(),
                padding: Padding::uniform(1),
            ) {
                Text(text: format_label)
                Select<String>(
                    items: format_items,
                    on_select: Handler::from(move |label: String| {
                        let format = match label.as_str() {
                            "Csv" => ReportFormat::Csv,
                            "Json" => ReportFormat::Json,
                            _ => ReportFormat::Text,
                        };
                        set_report_format(format);
                    }),
                    active: draft.focus == ReportFocus::Format,
                    default_index: Some(format_index),
                    height: Constraint::Length(3),
                )
                Text(text: destination_label)
                Input(
                    input: input,
                    placeholder: "report.txt".to_string(),
                    hide_cursor: draft.focus
                        != ReportFocus::Destination,
                )
                MultiSelect<String>(
                    items: option_items,
                    selected: options,
                    on_change: Handler::from(move |items: Vec<String>| {
                        let has = |label: &str| {
                            items.iter().any(|i| i == label)
                        };
                        set_report_options(
                            has("Same entries"),
                            has("Different entries"),
                            has("Orphan entries"),
                            has("Subdirectories"),
                            has("File details"),
                        );
                    }),
                    active: draft.focus == ReportFocus::Options,
                    default_index: Some(0),
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
    let marker = if focused {
        Span::styled("> ", theme.title)
    } else {
        Span::styled("  ", theme.dim)
    };
    Line::from(vec![marker, Span::raw(label)])
}
