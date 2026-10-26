// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Settings modal: the `,` entry point for the per-session toggles.
//!
//! The modal lists the three session settings (content comparison, hiding
//! identical entries, sync operation) with their current values; `Enter`
//! toggles or cycles the highlighted entry and keeps the modal open, so
//! several settings can be changed in a row, and `Esc` closes it. The
//! entries mirror the `C`, `F`, and `m` bindings — the modal is a
//! discoverable front for the same state, not a second source of truth.
//!
//! Like `TransferDialog` the component lives in the App shell and reads
//! the active session. Its handler owns only `Esc`; `j`/`k`/`Enter` are
//! reported as unbound so the `Select` child in the same layer navigates
//! and toggles.

use ratatui_kit::{
    crossterm::event::{Event, KeyEventKind},
    prelude::*,
    ratatui::{
        layout::{Constraint, Direction},
        text::Line,
        widgets::{ListItem, Padding},
    },
};

use super::theme::CocomoTheme;
use crate::{
    controller::{dispatch_settings_key, toggle_setting},
    state::{APP_STATE, SessionMode, SettingItem},
};

/// One rendered settings entry: the item to toggle and its current label.
#[derive(Clone, Debug)]
struct SettingRow {
    item: SettingItem,
    label: String,
}

impl From<SettingRow> for ListItem<'static> {
    fn from(row: SettingRow) -> Self {
        ListItem::from(row.label)
    }
}

/// Settings modal rendered above the active session.
#[component]
pub(super) fn SettingsModal(
    mut hooks: Hooks,
) -> impl Into<AnyElement<'static>> {
    let app_handle = hooks.use_atom(&APP_STATE);
    let theme = hooks.use_component_theme::<CocomoTheme>();

    // Build the rows from the active session's current settings, so the
    // labels always reflect the effective state.
    let rows: Vec<SettingRow> = app_handle
        .read()
        .active()
        .filter(|s| matches!(s.ui.mode, SessionMode::Settings))
        .map(|s| {
            let dir = s.dir();
            vec![
                SettingRow {
                    item: SettingItem::CompareContent,
                    label: format!(
                        "Compare content: {}",
                        on_off(dir.is_some_and(|d| d.compare_content))
                    ),
                },
                SettingRow {
                    item: SettingItem::HideSame,
                    label: format!(
                        "Hide identical: {}",
                        on_off(dir.is_some_and(|d| !d.display.show_same))
                    ),
                },
                SettingRow {
                    item: SettingItem::SyncOperation,
                    label: format!(
                        "Sync operation: {}",
                        dir.map(|d| d.sync_operation.label()).unwrap_or("-")
                    ),
                },
            ]
        })
        .unwrap_or_default();
    let open = !rows.is_empty();

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

            if dispatch_settings_key(key) {
                EventResult::Consumed
            } else {
                EventResult::Ignored
            }
        },
    );

    element!(
        Modal(
            open: open,
            // Reuse this component's layer; see `FilterPanel`.
            layer: Some(layer),
            width: Constraint::Length(48),
            height: Constraint::Length(9),
        ) {
            Border(
                flex_direction: Direction::Vertical,
                border_style: theme.border,
                top_title: Line::from(" Settings ")
                    .style(theme.title)
                    .centered(),
                bottom_title: Line::from(
                    " j/k select · Enter toggle · Esc close ",
                )
                .style(theme.dim)
                .centered(),
                padding: Padding::uniform(1),
            ) {
                Select<SettingRow>(
                    items: rows,
                    on_select: Handler::from(move |row: SettingRow| {
                        toggle_setting(row.item);
                    }),
                    active: true,
                    default_index: Some(0),
                    height: Constraint::Fill(1),
                )
            }
        }
    )
}

/// Render a boolean setting as `on`/`off`.
fn on_off(value: bool) -> &'static str {
    if value { "on" } else { "off" }
}
