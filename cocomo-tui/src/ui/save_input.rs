// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Save-session name prompt: the `s` entry point for saving a session.
//!
//! The prompt shows the single-line name buffer of the active session's
//! [`SessionMode::SaveSession`], pre-filled with the tab title. Unlike a
//! `SearchInput`, it needs no activation re-press: the component is only
//! mounted while the mode is active, so its exclusive input layer is open
//! from the first frame and the prompt is focused and editable right away
//! (the focus fix for issue #2). All keys are forwarded to
//! [`crate::controller::dispatch_save_key`], which edits the buffer,
//! cancels on `Esc`, and submits on `Enter`.
//!
//! The child `Input` is render-only (a controlled `tui_input` display);
//! the handler above owns the keys, mirroring the `FilterPanel` idiom.

use ratatui_kit::{
    crossterm::event::{Event, KeyEventKind},
    prelude::*,
    ratatui::{
        layout::{Constraint, Direction},
        text::Line,
    },
};

use super::{frame::hint_line, theme::CocomoTheme};
use crate::{
    controller::dispatch_save_key,
    state::{APP_STATE, SessionMode},
};

/// Single-row save-name prompt rendered in place of the status bar.
#[component]
pub(super) fn SaveInput(mut hooks: Hooks) -> impl Into<AnyElement<'static>> {
    let app_handle = hooks.use_atom(&APP_STATE);
    let theme = hooks.use_component_theme::<CocomoTheme>();

    // Resolve the name buffer from the active session; cloning it keeps the
    // read guard from living into the render below. The component is only
    // mounted while the mode is active, so the layer is open from the very
    // first frame.
    let (name, cursor): (String, usize) =
        app_handle.read().active().map_or_else(
            || (String::new(), 0),
            |s| match &s.ui.mode {
                SessionMode::SaveSession { name, cursor } => {
                    (name.clone(), *cursor)
                }
                _ => (String::new(), 0),
            },
        );

    let layer = hooks.use_input_layer(true, true);
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

            // The prompt owns its keys; anything it does not claim (e.g.
            // mouse events are filtered out above) falls through.
            if dispatch_save_key(key) {
                EventResult::Consumed
            } else {
                EventResult::Ignored
            }
        },
    );

    element!(
        Border(
            height: Constraint::Length(3),
            flex_direction: Direction::Vertical,
            border_style: theme.border,
            top_title: Line::from(" Save session ")
                .style(theme.title)
                .centered(),
            bottom_title: hint_line(&[("Enter", "save"), ("Esc", "cancel")])
                .style(theme.dim)
                .centered(),
        ) {
            Input(
                input: tui_input::Input::new(name).with_cursor(cursor),
                placeholder: "Session name...".to_string(),
            )
        }
    )
    .into_any()
}

#[cfg(test)]
mod tests {
    use cocomo_lib::Endpoint;
    use ratatui_kit::{ratatui::buffer::Buffer, test_util::render_frames};

    use super::*;
    use crate::{
        state::{AppState, SessionView},
        ui::theme::cocomo_palette,
    };

    /// A dir session whose only content is the given open save prompt.
    fn app_with_name(name: &str) -> AppState {
        let mut session = SessionView::new_dir_compare(
            Endpoint::local("/left"),
            Endpoint::local("/right"),
            false,
        );
        session.ui.mode = SessionMode::SaveSession {
            name: name.into(),
            cursor: 0,
        };
        let mut app = AppState::new();
        app.add_session(session);
        app
    }

    /// Render the open prompt into a buffer sized to its fixed layout.
    fn render(name: &str) -> Buffer {
        APP_STATE.set(app_with_name(name));
        render_frames(
            element!(PaletteProvider(palette: cocomo_palette()) {
                SaveInput()
            }),
            40,
            3,
            1,
        )
    }

    /// The prompt's single content row (between the borders), as text.
    fn input_row(buf: &Buffer) -> String {
        (1..buf.area.width - 1)
            .map(|x| {
                buf[(x, 1)]
                    .symbol()
                    .chars()
                    .next()
                    .unwrap_or(' ')
                    .to_string()
            })
            .collect()
    }

    #[test]
    fn prompt_shows_the_preset_name_and_title() {
        let buf = render("left vs right");
        let title: String = (1..buf.area.width - 1)
            .map(|x| {
                buf[(x, 0)]
                    .symbol()
                    .chars()
                    .next()
                    .unwrap_or(' ')
                    .to_string()
            })
            .collect();
        assert!(title.contains("Save session"), "got: {title:?}");
        let row = input_row(&buf);
        assert!(row.contains("left vs right"), "got: {row:?}");
        // The bottom border carries the key hints.
        let bottom: String = (1..buf.area.width - 1)
            .map(|x| {
                buf[(x, 2)]
                    .symbol()
                    .chars()
                    .next()
                    .unwrap_or(' ')
                    .to_string()
            })
            .collect();
        assert!(bottom.contains("Enter"), "got: {bottom:?}");
        assert!(bottom.contains("Esc"), "got: {bottom:?}");
    }

    #[test]
    fn empty_buffer_falls_back_to_the_placeholder() {
        let buf = render("");
        let row = input_row(&buf);
        assert!(row.contains("Session name"), "got: {row:?}");
    }
}
