// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Session tab bar showing open comparison sessions.

use ratatui_kit::{
    prelude::*,
    ratatui::{
        layout::Constraint,
        text::{Line, Span},
        widgets::Tabs,
    },
};

use super::theme::CocomoTheme;

/// Props for [`TabBar`].
#[derive(Default, Props)]
pub(super) struct TabBarProps {
    /// Titles of the session tabs.
    pub(super) tab_titles: Vec<Line<'static>>,
    /// Index of the active session tab.
    pub(super) active_index: usize,
}

/// Session tab bar showing open comparison sessions.
///
/// Rendered borderless on a single row to keep the chrome compact. When no
/// sessions are open the bar collapses to zero height, so the welcome
/// screen gets the full main view area.
#[component]
pub(super) fn TabBar(
    props: &TabBarProps,
    hooks: Hooks,
) -> impl Into<AnyElement<'static>> {
    let theme = hooks.use_component_theme::<CocomoTheme>();
    let tab_tabs = Tabs::new(props.tab_titles.clone())
        .select(props.active_index)
        .highlight_style(theme.title)
        .divider(Span::raw(" | "));
    let height = Constraint::Length(u16::from(!props.tab_titles.is_empty()));

    element!(
        View(height: height) {
            widget(tab_tabs)
        }
    )
}
