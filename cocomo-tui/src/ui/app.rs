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
    crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers},
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
    welcome::WelcomeScreen,
};
use crate::{
    controller,
    state::{APP_STATE, SessionMode, SessionType},
};

/// Root component: subscribes to global state, registers global key handlers,
/// and renders the menu bar, tab bar, and main content.
#[component]
pub(crate) fn App(mut hooks: Hooks) -> impl Into<AnyElement<'static>> {
    let app_state = hooks.use_atom(&APP_STATE);
    let mut exit = hooks.use_exit();

    // Register global key handler: Ctrl+W (close tab), Ctrl+Tab (switch
    // tabs), Left/Right (menu nav), q (quit). All state mutations go
    // through the controller; the atom handle is only used for reads.
    hooks.use_event_handler(
        EventScope::Current,
        EventPriority::Normal,
        move |event| {
            const MENU_COUNT: usize = 6; // Sessions, Compare, Sync, View, Filter, Help

            let Event::Key(key) = event else {
                return EventResult::Ignored;
            };
            if key.kind != KeyEventKind::Press {
                return EventResult::Ignored;
            }

            // Ctrl+W — close active tab; quit when it was the last one.
            if key.code == KeyCode::Char('w')
                && key.modifiers.contains(KeyModifiers::CONTROL)
            {
                if controller::close_active_tab() {
                    exit();
                }
                return EventResult::Consumed;
            }

            // Ctrl+Tab — switch tabs (Shift reverses direction).
            if key.code == KeyCode::Tab
                && key.modifiers.contains(KeyModifiers::CONTROL)
            {
                let is_shift = key.modifiers.contains(KeyModifiers::SHIFT);
                controller::switch_tab(if is_shift { -1 } else { 1 });
                return EventResult::Consumed;
            }

            // Left/Right arrow keys — navigate the menu bar.
            if key.modifiers == KeyModifiers::NONE {
                match key.code {
                    KeyCode::Left => {
                        let current = app_state.read().menu_selection;
                        controller::set_menu_selection(if current > 0 {
                            current - 1
                        } else {
                            MENU_COUNT - 1
                        });
                        return EventResult::Consumed;
                    }
                    KeyCode::Right => {
                        let current = app_state.read().menu_selection;
                        controller::set_menu_selection(
                            (current + 1) % MENU_COUNT,
                        );
                        return EventResult::Consumed;
                    }
                    _ => {}
                }
            }

            // q — quit (unless the active session is in filter mode, where
            // the session's input layer handles the key).
            if key.code == KeyCode::Char('q') || key.code == KeyCode::Char('Q')
            {
                let quit = match app_state.read().active() {
                    Some(session) => session.mode != SessionMode::Filter,
                    None => true,
                };
                if quit {
                    exit();
                    return EventResult::Consumed;
                }
            }

            EventResult::Ignored
        },
    );

    // Read current state for rendering the menu and tab bars.
    let app = app_state.read();
    let menu_selection = app.menu_selection;
    let active_index = app.active_index();
    let active_session_type = app.active().map(|s| s.session_type);

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
                match active_session_type {
                    Some(SessionType::DirCompare) => { DirCompareView() }
                    Some(SessionType::TextCompare) => { TextCompareView() }
                    None => { WelcomeScreen() }
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
        }
    )
}
