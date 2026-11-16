// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Save-overwrite prompt: the name-clash step of saving a session.
//!
//! When a save finds that `{name}.toml` already exists, the controller
//! puts the session into [`SessionMode::SaveOverwrite`]; this component
//! turns that mode into an [`OptionList`] with the two decisions:
//! `[y]`/`Enter` on Overwrite replaces the saved session, `n`/`Esc` (or
//! `Enter` on Change name) take the name back to the focused input. The
//! cursor starts on Change name, so a bare `Enter` cannot replace the
//! saved file by accident.
//!
//! Like `SyncConfirmModal` the component lives in the App shell and reads
//! the active session, so one implementation serves every tab. It owns an
//! exclusive input layer while open: its handler runs in that layer with
//! `EventPriority::High` and owns `y`/`n`/`Esc` (the
//! `KeyScope::SaveOverwrite` bindings), while `j`/`k`/`Enter` reach the
//! `OptionList` child in the same layer.

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
    controller::{
        change_save_name, confirm_overwrite, dispatch_save_overwrite_key,
    },
    state::{APP_STATE, SessionMode},
};

/// Save-overwrite prompt rendered above the active session.
#[component]
pub(super) fn SaveOverwriteModal(
    mut hooks: Hooks,
) -> impl Into<AnyElement<'static>> {
    let app_handle = hooks.use_atom(&APP_STATE);
    let theme = hooks.use_component_theme::<CocomoTheme>();

    // Resolve the clashed name from the active session; the prompt only
    // opens while a save reported a conflict.
    let name = app_handle.read().active().and_then(|s| match &s.ui.mode {
        SessionMode::SaveOverwrite { name } => Some(name.clone()),
        _ => None,
    });
    let open = name.is_some();

    // All hooks must run on every frame, open or not, so the layer is
    // minted unconditionally and simply inactive while `open` is false.
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

            // The prompt owns `y`/`n`/`Esc` (via the keymap); an unowned
            // key stays unclaimed here, so the `OptionList` child in the
            // same layer gets `j`/`k`/`Enter`.
            if dispatch_save_overwrite_key(key) {
                EventResult::Consumed
            } else {
                EventResult::Ignored
            }
        },
    );

    let Some(name) = name else {
        // Closed: a zero-sized `Modal` instead of a `Fragment` whose default
        // `Fill(1)` height would steal the main view's rows. Returning early
        // also unmounts the option list, so every open starts on Change name.
        return element!(Modal(open: false)).into_any();
    };

    // The clashed name is quoted so the message reads unambiguously even
    // for names with spaces.
    let message =
        Line::from(format!("A session named \"{name}\" already exists."));

    // Two border rows plus the padding around the message and the two
    // decision rows.
    let height = 4 + 1 + 2;

    element!(
        Modal(
            open: true,
            // Reuse this component's layer instead of letting `Modal` mint
            // its own, which would deafen the handler above.
            layer: Some(layer),
            // Wide enough for the message with a typical session name.
            width: Constraint::Length(44),
            height: Constraint::Length(height),
        ) {
            Border(
                flex_direction: Direction::Vertical,
                border_style: theme.border,
                top_title: Line::from(" Session exists ")
                    .style(theme.title)
                    .centered(),
                bottom_title: hint_line(&[
                    ("j/k", "select"),
                    ("Enter", "confirm"),
                    ("Esc", "change name"),
                ])
                .style(theme.dim)
                .centered(),
                padding: Padding::uniform(1),
            ) {
                Text(text: message)
                OptionList<bool>(
                    items: vec![
                        OptionItem::new(true, "Overwrite").key("y"),
                        OptionItem::new(false, "Change name").key("n"),
                    ],
                    on_select: Handler::from(move |overwrite: bool| {
                        if overwrite {
                            confirm_overwrite();
                        } else {
                            change_save_name();
                        }
                    }),
                    // Highlighting Change name keeps an accidental `Enter`
                    // from replacing the saved session.
                    default_index: Some(1),
                    // The two decision rows; the message above keeps the
                    // remaining row of the modal.
                    height: Constraint::Length(2),
                )
            }
        }
    )
    .into_any()
}

#[cfg(test)]
mod tests {
    use cocomo_lib::Endpoint;
    use ratatui_kit::test_util::render_frames;

    use super::*;
    use crate::{
        state::{AppState, SessionView},
        ui::theme::cocomo_palette,
    };

    /// A dir session whose only content is the given open overwrite
    /// prompt.
    fn app_with_clash(name: &str) -> AppState {
        let mut session = SessionView::new_dir_compare(
            Endpoint::local("/left"),
            Endpoint::local("/right"),
            false,
        );
        session.ui.mode = SessionMode::SaveOverwrite { name: name.into() };
        let mut app = AppState::new();
        app.add_session(session);
        app
    }

    /// The whole modal as text, row by row, for substring assertions.
    fn render(app: AppState) -> String {
        APP_STATE.set(app);
        let buf = render_frames(
            element!(PaletteProvider(palette: cocomo_palette()) {
                SaveOverwriteModal()
            }),
            60,
            7,
            1,
        );
        (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| {
                        buf[(x, y)].symbol().chars().next().unwrap_or(' ')
                    })
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn prompt_names_the_clash_and_offers_the_two_decisions() {
        let screen = render(app_with_clash("demo"));
        assert!(screen.contains("Session exists"), "got:\n{screen}");
        assert!(
            screen.contains("A session named \"demo\" already exists."),
            "got:\n{screen}"
        );
        assert!(screen.contains("Overwrite"), "got:\n{screen}");
        assert!(screen.contains("Change name"), "got:\n{screen}");
    }

    #[test]
    fn prompt_is_closed_without_the_mode() {
        let mut app = AppState::new();
        app.add_session(SessionView::new_dir_compare(
            Endpoint::local("/left"),
            Endpoint::local("/right"),
            false,
        ));
        let screen = render(app);
        // A closed `Modal` renders nothing; no hint of a prompt leaks out.
        assert!(!screen.contains("Session exists"), "got:\n{screen}");
    }
}
