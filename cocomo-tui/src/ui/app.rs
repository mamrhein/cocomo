// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Root `App` component: menu bar, tab bar, active view, and error modal.

use ratatui_kit::{
    crossterm::event::{Event, KeyEventKind, KeyModifiers},
    prelude::*,
    ratatui::{
        layout::{Constraint, Direction},
        text::Line,
    },
};

use super::{
    dir_compare::DirCompareView,
    menu_bar::{MENU_TITLES, MenuBar},
    tab_bar::TabBar,
    text_diff::TextCompareView,
    transfer_dialog::TransferDialog,
    welcome::WelcomeScreen,
};
use crate::{
    controller,
    keys::{self, Cmd, KeyScope},
    state::{APP_STATE, SessionContent},
};

/// Which view the main content area renders, derived from the active
/// session's [`SessionContent`] (the enum is the single source of truth for
/// the session type).
#[derive(Clone, Copy)]
enum ActiveView {
    Dir,
    Text,
    Welcome,
}

/// Root component: subscribes to global state, registers global key handlers,
/// and renders the menu bar, tab bar, and main content.
#[component]
pub(crate) fn App(mut hooks: Hooks) -> impl Into<AnyElement<'static>> {
    let app_state = hooks.use_atom(&APP_STATE);
    let mut exit = hooks.use_exit();

    // Register the global key handler. Which keys the shell owns is defined
    // by the `Global` scope of the keymap (`keys::KEYMAP`); the help bar is
    // generated from the same table. All state mutations go through the
    // controller; the atom handle is only used for reads.
    hooks.use_event_handler(
        EventScope::Current,
        EventPriority::Normal,
        move |event| {
            let Event::Key(key) = event else {
                return EventResult::Ignored;
            };
            if key.kind != KeyEventKind::Press {
                return EventResult::Ignored;
            }

            let Some(cmd) = keys::lookup(KeyScope::Global, key) else {
                return EventResult::Ignored;
            };

            match cmd {
                // Close active tab; quit when it was the last one.
                Cmd::CloseTab => {
                    if controller::close_active_tab() {
                        exit();
                    }
                }
                // Switch tabs (Shift reverses direction).
                Cmd::SwitchTab => {
                    let is_shift = key.modifiers.contains(KeyModifiers::SHIFT);
                    controller::switch_tab(if is_shift { -1 } else { 1 });
                }
                Cmd::MenuPrev => {
                    let current = app_state.read().menu_selection;
                    controller::set_menu_selection(if current > 0 {
                        current - 1
                    } else {
                        MENU_TITLES.len() - 1
                    });
                }
                Cmd::MenuNext => {
                    let current = app_state.read().menu_selection;
                    controller::set_menu_selection(
                        (current + 1) % MENU_TITLES.len(),
                    );
                }
                Cmd::Quit => {
                    // Text-entry modes own `q` for their input fields;
                    // their input layers normally block this handler
                    // already, so the guard is belt and braces.
                    if app_state
                        .read()
                        .active()
                        .is_some_and(|s| s.ui.mode.is_editing())
                    {
                        return EventResult::Ignored;
                    }
                    exit();
                }
                Cmd::ToggleHelp => {
                    controller::toggle_help();
                }
                // Session keys are owned by the session view components.
                _ => return EventResult::Ignored,
            }

            EventResult::Consumed
        },
    );

    // Read current state for rendering the menu and tab bars.
    let app = app_state.read();
    let menu_selection = app.menu_selection;
    let active_index = app.active_index();
    let active_view = match app.active().map(|s| &s.content) {
        Some(SessionContent::Dir(_)) => ActiveView::Dir,
        Some(SessionContent::Text(_)) => ActiveView::Text,
        None => ActiveView::Welcome,
    };

    // Extract error message for the AlertModal (cloned to avoid holding the
    // read guard across element! rendering).
    let error_message = app.error_message.clone();

    // Build menu bar tab titles.
    let menu_titles: Vec<Line<'static>> =
        MENU_TITLES.iter().map(|s| Line::from(*s)).collect();

    // Build session tab titles.
    let tab_titles: Vec<Line<'static>> = app
        .sessions
        .iter()
        .map(|s| Line::from(s.tab_title().to_string()))
        .collect();

    drop(app);

    element!(
        View(flex_direction: Direction::Vertical) {
            MenuBar(menu_titles: menu_titles, selected: menu_selection)
            TabBar(tab_titles: tab_titles, active_index: active_index)
            View(height: Constraint::Min(1)) {
                match active_view {
                    ActiveView::Dir => { DirCompareView() }
                    ActiveView::Text => { TextCompareView() }
                    ActiveView::Welcome => { WelcomeScreen() }
                }
            }
            AlertModal(
                open: error_message.is_some(),
                title: Line::from(" Error "),
                message: error_message.unwrap_or_default(),
                close_hint: Line::from("Enter / Esc").centered(),
                on_close: move |_: ()| {
                    controller::dismiss_error();
                },
            )
            // Copy/delete confirmation, driven by the active session's
            // transfer mode.
            TransferDialog()
        }
    )
}
