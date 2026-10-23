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
    prelude::*,
    ratatui::{
        layout::Constraint,
        text::{Line, Span},
        widgets::{Block, Borders, Tabs},
    },
};

use super::theme::CocomoTheme;

/// Menu bar item labels, in order: Sessions, Compare, Sync, View, Filter,
/// Help.
pub(super) const MENU_TITLES: [&str; 6] =
    ["Sessions", "Compare", "Sync", "View", "Filter", "Help"];

/// Props for [`MenuBar`].
#[derive(Default, Props)]
pub(super) struct MenuBarProps {
    /// Titles of the menu items.
    pub(super) menu_titles: Vec<Line<'static>>,
    /// Index of the currently selected menu item.
    pub(super) selected: usize,
}

/// Top menu bar with tab-like navigation items (Sessions, Compare, Sync,
/// View, Filter, Help).
#[component]
pub(super) fn MenuBar(
    props: &MenuBarProps,
    hooks: Hooks,
) -> impl Into<AnyElement<'static>> {
    let theme = hooks.use_component_theme::<CocomoTheme>();
    let menu_tabs = Tabs::new(props.menu_titles.clone())
        .select(props.selected)
        .highlight_style(theme.title)
        .divider(Span::raw(" | "))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(theme.border),
        );

    element!(
        View(height: Constraint::Length(3)) {
            widget(menu_tabs)
        }
    )
}
