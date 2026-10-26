// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Top menu bar with tab-like navigation items.

use ratatui_kit::{
    crossterm::event::MouseButton,
    prelude::*,
    ratatui::{
        layout::Constraint,
        text::{Line, Span},
        widgets::Tabs,
    },
};

use super::{frame::tabs_hit_item, mouse_enabled, theme::CocomoTheme};

/// Menu bar item labels, in order: Sessions, Compare, Sync, View, Filter,
/// Help.
pub(super) const MENU_TITLES: [&str; 6] =
    ["Sessions", "Compare", "Sync", "View", "Filter", "Help"];

/// Width of the `" | "` divider between menu items.
const DIVIDER_WIDTH: u16 = 3;

/// Props for [`MenuBar`].
#[derive(Default, Props)]
pub(super) struct MenuBarProps {
    /// Titles of the menu items.
    pub(super) menu_titles: Vec<Line<'static>>,
    /// Index of the currently selected menu item.
    pub(super) selected: usize,
    /// Called with the item index when a menu item is clicked.
    pub(super) on_select: Handler<'static, usize>,
}

/// Top menu bar with tab-like navigation items (Sessions, Compare, Sync,
/// View, Filter, Help).
///
/// Rendered borderless on a single row to keep the chrome compact; the
/// selected item is highlighted with the theme's title style. Clicking an
/// item activates its command (the keyboard counterparts are the existing
/// keybindings, so no focus model is needed here).
#[component]
pub(super) fn MenuBar(
    props: &mut MenuBarProps,
    mut hooks: Hooks,
) -> impl Into<AnyElement<'static>> {
    let theme = hooks.use_component_theme::<CocomoTheme>();

    if mouse_enabled() {
        let mut on_select = props.on_select.take();
        let titles = props.menu_titles.clone();
        hooks.use_pointer(PointerOptions::default(), move |ev| {
            let PointerKind::Click {
                button: MouseButton::Left,
                ..
            } = ev.kind
            else {
                return EventResult::Ignored;
            };
            // `position` shares the draw-area coordinates of `area`, so
            // the x is made bar-local before the item mapping.
            let item = ev
                .position
                .x
                .checked_sub(ev.area.x)
                .and_then(|x| tabs_hit_item(x, &titles, DIVIDER_WIDTH));
            match item {
                Some(index) => {
                    on_select(index);
                    EventResult::Consumed
                }
                None => EventResult::Ignored,
            }
        });
    }

    let menu_tabs = Tabs::new(props.menu_titles.clone())
        .select(props.selected)
        .highlight_style(theme.title)
        .divider(Span::raw(" | "));

    element!(
        View(height: Constraint::Length(1)) {
            widget(menu_tabs)
        }
    )
}
