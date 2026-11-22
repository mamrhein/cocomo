// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Session info modal: the `I` entry point for the session summary.
//!
//! Mirrors the GUI's `open_folder_info_dialog` and `open_info_dialog`: a
//! read-only dialog listing the compared pair's display paths plus, for a
//! folder session, the status counts over the entries the filters show
//! ([`SessionView::visible_summary`]), the shown status groups, the hidden
//! items state and the active name filter; for a text session the line
//! counts and the difference statistics derived from the merged rows.
//! `Enter` and `Esc` close it.
//!
//! Like the settings modal the component lives in the App shell and reads
//! the active session; its handler owns the closing keys through the
//! exclusive input layer, so background commands stay unreachable while
//! the modal is open.

use cocomo_lib::MergedRowStatus;
use ratatui_kit::{
    crossterm::event::{Event, KeyEventKind},
    prelude::*,
    ratatui::{
        layout::{Constraint, Direction},
        text::Line,
        widgets::Padding,
    },
};

use super::{frame::hint_line, theme::CocomoTheme};
use crate::{
    controller::dispatch_info_key,
    state::{APP_STATE, DirView, SessionMode, SessionView, StatusFilter},
};

/// Width of the modal; wide enough for typical pair paths, with longer
/// values truncated to fit (see [`row`]).
const MODAL_WIDTH: u16 = 64;

/// Content columns inside the modal's border and padding.
const INNER_WIDTH: usize = (MODAL_WIDTH - 4) as usize;

/// Session info modal rendered above the active session.
#[component]
pub(super) fn SessionInfoModal(
    mut hooks: Hooks,
) -> impl Into<AnyElement<'static>> {
    let app_handle = hooks.use_atom(&APP_STATE);
    let theme = hooks.use_component_theme::<CocomoTheme>();

    // Build the rows from the active session each frame, so a reload or
    // filter change behind the modal is reflected as soon as it repaints.
    let rows: Option<Vec<Line<'static>>> = {
        let app = app_handle.read();
        app.active()
            .filter(|s| matches!(s.ui.mode, SessionMode::Info))
            .map(info_rows)
    };
    let open = rows.is_some();

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

            // The modal owns its closing keys (via the keymap); every
            // other key is ignored here and swallowed by the layer.
            if dispatch_info_key(key) {
                EventResult::Consumed
            } else {
                EventResult::Ignored
            }
        },
    );

    let Some(rows) = rows else {
        // Closed: a zero-sized `Modal` instead of a `Fragment` whose default
        // `Fill(1)` height would steal the main view's rows.
        return element!(Modal(open: false)).into_any();
    };

    element!(
        Modal(
            open: true,
            // Reuse this component's layer; see `SettingsModal`. The
            // height follows the row count (up to 9 for folders, 8 for
            // text) plus the border, padding and titles.
            layer: Some(layer),
            width: Constraint::Length(MODAL_WIDTH),
            height: Constraint::Length(rows.len() as u16 + 7),
        ) {
            Border(
                flex_direction: Direction::Vertical,
                border_style: theme.border,
                top_title: Line::from(" Session info ")
                    .style(theme.title)
                    .centered(),
                bottom_title: hint_line(&[
                    ("Enter", "close"),
                    ("Esc", "close"),
                ])
                .style(theme.dim)
                .centered(),
                padding: Padding::uniform(1),
            ) {
                for (i, row) in rows.iter().enumerate() {
                    Text(key: i, text: row.clone())
                }
            }
        }
    )
    .into_any()
}

/// The info rows for `session`: pair paths first, then the summary
/// specific to the session kind.
fn info_rows(session: &SessionView) -> Vec<Line<'static>> {
    let mut rows = vec![
        row(&format!("Left:  {}", session.core.left.display())),
        row(&format!("Right: {}", session.core.right.display())),
    ];
    if let Some(dir) = session.dir() {
        match session.visible_summary() {
            Some(summary) => {
                rows.push(row(&format!(
                    "Entries: {} ({} files, {} directories)",
                    summary.total, summary.files, summary.dirs
                )));
                rows.push(row(&format!("Identical: {}", summary.same)));
                rows.push(row(&format!("Different: {}", summary.different)));
                rows.push(row(&format!("Orphans: {}", summary.orphans)));
            }
            None => rows.push(row("Comparison: not loaded yet")),
        }
        rows.push(row(&format!("Filters: {}", shown_status_groups(dir))));
        rows.push(row(&format!(
            "Hidden items: {}",
            if dir.shows_hidden() {
                "shown"
            } else {
                "hidden"
            }
        )));
        if let Some(filter) = &session.ui.name_filter {
            rows.push(row(&format!("Name filter: {}", filter.pattern)));
        }
    } else if let Some(text) = session.text() {
        match text.text_diff.as_ref() {
            Some(diff) => {
                let mut insertions = 0_usize;
                let mut deletions = 0_usize;
                let mut changes = 0_usize;
                for line in &text.merged_rows {
                    match line.status {
                        MergedRowStatus::Inserted => insertions += 1,
                        MergedRowStatus::Deleted => deletions += 1,
                        MergedRowStatus::Changed => changes += 1,
                        MergedRowStatus::Same => {}
                    }
                }
                rows.push(row(&format!(
                    "Left lines:  {}",
                    diff.left_line_count
                )));
                rows.push(row(&format!(
                    "Right lines: {}",
                    diff.right_line_count
                )));
                rows.push(row(&format!(
                    "Difference sections: {}",
                    diff.group_starts().len()
                )));
                rows.push(row(&format!("Inserted lines: {insertions}")));
                rows.push(row(&format!("Deleted lines: {deletions}")));
                rows.push(row(&format!("Changed lines: {changes}")));
            }
            None => rows.push(row("Comparison: not loaded yet")),
        }
    }
    rows
}

/// The umbrella status groups currently shown, as the `Filters:` value.
///
/// The TUI has no GUI filter presets, so the row lists the shown groups
/// directly (`same`, `different`, `orphans`), or `none` when every group
/// is switched off.
fn shown_status_groups(dir: &DirView) -> String {
    let shown = [
        (StatusFilter::Equal, "same"),
        (StatusFilter::Different, "different"),
        (StatusFilter::Orphans, "orphans"),
    ]
    .into_iter()
    .filter(|(filter, _)| filter.is_shown(dir.display.show_statuses))
    .map(|(_, label)| label)
    .collect::<Vec<_>>();
    if shown.is_empty() {
        "none".into()
    } else {
        shown.join(", ")
    }
}

/// One modal content row, truncated with an ellipsis when it would
/// exceed the modal's inner width.
fn row(text: &str) -> Line<'static> {
    if text.chars().count() <= INNER_WIDTH {
        return Line::from(text.to_string());
    }
    let trimmed: String =
        text.chars().take(INNER_WIDTH - 1).collect::<String>();
    Line::from(format!("{trimmed}\u{2026}"))
}
