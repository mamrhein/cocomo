// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Session browser modal: the `O` entry point for saved sessions.
//!
//! The modal lists the saved session files in an [`OptionList`]; `Enter`
//! loads the highlighted session, `n` closes the browser and enters the
//! save-name prompt to create a new one, and `Esc` closes it. The list is
//! filled asynchronously by the `ListSessions` action the browser
//! dispatches on open, so the modal shows a placeholder until the listing
//! arrives.
//!
//! Like `TransferDialog` the component lives in the App shell and reads
//! the active session. Its handler runs in the modal's exclusive input
//! layer with `EventPriority::High` and owns only `Esc` and `n`;
//! `j`/`k`/`Enter` are reported as unbound so the `OptionList` child in the
//! same layer navigates and confirms.

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
    controller::{dispatch_browser_key, load_saved_session},
    state::{APP_STATE, SessionMode},
};

/// Session browser modal rendered above the active session.
#[component]
pub(super) fn SessionBrowser(
    mut hooks: Hooks,
) -> impl Into<AnyElement<'static>> {
    let app_handle = hooks.use_atom(&APP_STATE);
    let theme = hooks.use_component_theme::<CocomoTheme>();

    // Resolve the saved-session list from the active session.
    let sessions: Option<Vec<String>> =
        app_handle.read().active().and_then(|s| match &s.ui.mode {
            SessionMode::SessionBrowser { saved_sessions } => {
                Some(saved_sessions.clone())
            }
            _ => None,
        });
    let open = sessions.is_some();

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

            // The browser owns `Esc` and `n` (via the keymap); an unowned
            // key stays unclaimed here, so the `OptionList` child in the
            // same layer gets `j`/`k`/`Enter`.
            if dispatch_browser_key(key) {
                EventResult::Consumed
            } else {
                EventResult::Ignored
            }
        },
    );

    let Some(items) = sessions else {
        // Closed: a zero-sized `Modal` instead of a `Fragment` whose default
        // `Fill(1)` height would steal the main view's rows. Returning early
        // also unmounts the option list, so every open starts at the top.
        return element!(Modal(open: false)).into_any();
    };

    let items: Vec<OptionItem<String>> = items
        .into_iter()
        .map(|name| OptionItem::new(name.clone(), name))
        .collect();

    element!(
        Modal(
            open: true,
            // Reuse this component's layer; see `FilterPanel`.
            layer: Some(layer),
            width: Constraint::Length(48),
            height: Constraint::Length(12),
        ) {
            Border(
                flex_direction: Direction::Vertical,
                border_style: theme.border,
                top_title: Line::from(" Sessions ")
                    .style(theme.title)
                    .centered(),
                bottom_title: hint_line(&[
                    ("j/k", "select"),
                    ("Enter", "load"),
                    ("n", "new"),
                    ("Esc", "cancel"),
                ])
                .style(theme.dim)
                .centered(),
                padding: Padding::uniform(1),
            ) {
                OptionList<String>(
                    items: items,
                    on_select: Handler::from(move |name: String| {
                        load_saved_session(name);
                    }),
                    default_index: Some(0),
                    empty_message: Line::from("No saved sessions yet."),
                )
            }
        }
    )
    .into_any()
}
