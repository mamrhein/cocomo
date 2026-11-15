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
//! The modal lists the settings of the active session kind with their
//! current values in the hint column of an [`OptionList`]: folder sessions
//! get content comparison and hiding identical entries, text sessions get
//! the three text rule toggles. `Enter` toggles the highlighted entry and
//! keeps the modal open, so several settings can be changed in a row, and
//! `Esc` closes it. The dir entries mirror the `C` and `F` bindings — the
//! modal is a discoverable front for the same state, not a second source
//! of truth.
//!
//! Like `TransferDialog` the component lives in the App shell and reads
//! the active session. Its handler owns only `Esc`; `j`/`k`/`Enter` are
//! reported as unbound so the `OptionList` child in the same layer
//! navigates and toggles.

use ratatui_kit::{
    crossterm::event::{Event, KeyEventKind},
    prelude::*,
    ratatui::{
        layout::{Constraint, Direction},
        text::Line,
        widgets::Padding,
    },
};

use super::{
    frame::hint_line,
    option_list::{OptionItem, OptionList},
    theme::CocomoTheme,
};
use crate::{
    controller::{dispatch_settings_key, toggle_setting},
    state::{APP_STATE, SessionMode, SettingItem},
};

/// Settings modal rendered above the active session.
#[component]
pub(super) fn SettingsModal(
    mut hooks: Hooks,
) -> impl Into<AnyElement<'static>> {
    let app_handle = hooks.use_atom(&APP_STATE);
    let theme = hooks.use_component_theme::<CocomoTheme>();

    // Build the rows from the active session's current settings, so the
    // hints always reflect the effective state. Only the entries that
    // apply to the session kind are listed, so no toggle is a silent
    // no-op; the list cursor stays on the same row across rebuilds.
    let rows: Option<Vec<OptionItem<SettingItem>>> = app_handle
        .read()
        .active()
        .filter(|s| matches!(s.ui.mode, SessionMode::Settings))
        .map(|s| {
            if let Some(dir) = s.dir() {
                vec![
                    OptionItem::new(
                        SettingItem::CompareContent,
                        "Compare content",
                    )
                    .hint(on_off(dir.compare_content)),
                    OptionItem::new(SettingItem::HideSame, "Hide identical")
                        .hint(on_off(!dir.shows_equal())),
                ]
            } else {
                let rules = s.text().map(|t| t.source.rules);
                vec![
                    OptionItem::new(
                        SettingItem::IgnoreWhitespace,
                        "Ignore whitespace",
                    )
                    .hint(on_off(rules.is_some_and(|r| r.ignore_whitespace))),
                    OptionItem::new(
                        SettingItem::SkipBlankLines,
                        "Skip blank lines",
                    )
                    .hint(on_off(rules.is_some_and(|r| r.skip_blank_lines))),
                    OptionItem::new(
                        SettingItem::SkipComments,
                        "Skip comments",
                    )
                    .hint(on_off(rules.is_some_and(|r| r.skip_comments))),
                ]
            }
        });
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

            // The modal owns `Esc` (via the keymap); an unowned key stays
            // unclaimed here, so the `OptionList` child in the same layer
            // gets `j`/`k`/`Enter`.
            if dispatch_settings_key(key) {
                EventResult::Consumed
            } else {
                EventResult::Ignored
            }
        },
    );

    let Some(items) = rows else {
        // Closed: a zero-sized `Modal` instead of a `Fragment` whose default
        // `Fill(1)` height would steal the main view's rows.
        return element!(Modal(open: false)).into_any();
    };

    element!(
        Modal(
            open: true,
            // Reuse this component's layer; see `FilterPanel`. The height
            // follows the entry count (2 rows for dir, 3 for text) plus
            // the border, padding and titles.
            layer: Some(layer),
            width: Constraint::Length(48),
            height: Constraint::Length(items.len() as u16 + 7),
        ) {
            Border(
                flex_direction: Direction::Vertical,
                border_style: theme.border,
                top_title: Line::from(" Settings ")
                    .style(theme.title)
                    .centered(),
                bottom_title: hint_line(&[
                    ("j/k", "select"),
                    ("Enter", "toggle"),
                    ("Esc", "cancel"),
                ])
                .style(theme.dim)
                .centered(),
                padding: Padding::uniform(1),
            ) {
                OptionList<SettingItem>(
                    items: items,
                    on_select: Handler::from(move |item: SettingItem| {
                        toggle_setting(item);
                    }),
                    default_index: Some(0),
                )
            }
        }
    )
    .into_any()
}

/// Render a boolean setting as `on`/`off`.
fn on_off(value: bool) -> &'static str {
    if value { "on" } else { "off" }
}
