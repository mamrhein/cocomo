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
//! The modal lists the saved session files in a `Select`; `Enter` loads
//! the highlighted session, `n` closes the browser and enters the
//! save-name prompt to create a new one, and `Esc` closes it. The list is
//! filled asynchronously by the `ListSessions` action the browser
//! dispatches on open, so the modal shows a placeholder until the listing
//! arrives.
//!
//! Like `TransferDialog` the component lives in the App shell and reads
//! the active session. Its handler runs in the modal's exclusive input
//! layer with `EventPriority::High` and owns only `Esc` and `n`;
//! `j`/`k`/`Enter` are reported as unbound so the `Select` child in the
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

use super::theme::CocomoTheme;
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

            if dispatch_browser_key(key) {
                EventResult::Consumed
            } else {
                EventResult::Ignored
            }
        },
    );

    let items = sessions.unwrap_or_default();

    element!(
        Modal(
            open: open,
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
                bottom_title: Line::from(
                    " j/k select · Enter load · n new · Esc close ",
                )
                .style(theme.dim)
                .centered(),
                padding: Padding::uniform(1),
            ) {
                Select<String>(
                    items: items,
                    on_select: Handler::from(move |name: String| {
                        load_saved_session(name);
                    }),
                    active: true,
                    default_index: Some(0),
                    empty_message: TextParagraph::from(
                        "No saved sessions yet.",
                    ),
                    height: Constraint::Fill(1),
                )
            }
        }
    )
}
