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
        widgets::{Block, Borders, Tabs},
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

/// Session tab bar showing open comparison sessions. When no sessions are
/// open, an empty bordered block is rendered.
#[component]
pub(super) fn TabBar(
    props: &TabBarProps,
    hooks: Hooks,
) -> impl Into<AnyElement<'static>> {
    let theme = hooks.use_component_theme::<CocomoTheme>();
    // One `Tabs` value serves both cases: with no titles it renders just
    // the empty bordered block, so the branches do not need to differ.
    let tab_tabs = Tabs::new(props.tab_titles.clone())
        .select(props.active_index)
        .highlight_style(theme.title)
        .divider(Span::raw(" | "))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(theme.border),
        );

    element!(
        View(height: Constraint::Length(3)) {
            widget(tab_tabs)
        }
    )
}
