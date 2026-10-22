// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! ratatui-kit UI components: the root `App` shell, the menu and tab bars,
//! the welcome screen, the two comparison views, and the hooks they share.

pub(crate) mod app;
mod dir_compare;
mod menu_bar;
mod rows;
mod style;
mod tab_bar;
mod text_diff;
mod welcome;

use ratatui_kit::prelude::*;

/// Mirror the session-owned table selection into a component-local
/// `State<TableState>` for the `Table` component.
///
/// The session (`SessionView::table_state`) is the single owner of the
/// selection: key handling and async completions mutate it under the
/// `APP_STATE` write lock. The `Table` component, however, only accepts a
/// reactive `State` handle, so each view mirrors the owner's value into
/// this handle on every render — the session value always wins, and the
/// handle exists purely to feed the widget.
///
/// `write_no_update` is deliberate: the atom write that changed the
/// selection already woke the subscribed component, so the mirror must
/// not trigger a second redraw.
pub(super) fn use_session_table_state(
    hooks: &mut Hooks<'_, '_>,
    selected: Option<usize>,
) -> State<TableState> {
    let table_state = hooks.use_state(TableState::default);
    if table_state.read().selected() != selected {
        table_state.write_no_update().select(selected);
    }
    table_state
}
