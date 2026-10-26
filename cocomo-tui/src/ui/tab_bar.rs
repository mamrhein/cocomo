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
    crossterm::event::MouseButton,
    prelude::*,
    ratatui::{
        layout::Constraint,
        text::{Line, Span},
        widgets::Tabs,
    },
};

use super::{frame::tabs_hit_item, mouse_enabled, theme::CocomoTheme};

/// Width of the `" | "` divider between session tabs.
const DIVIDER_WIDTH: u16 = 3;

/// Props for [`TabBar`].
#[derive(Default, Props)]
pub(super) struct TabBarProps {
    /// Titles of the session tabs.
    pub(super) tab_titles: Vec<Line<'static>>,
    /// Index of the active session tab.
    pub(super) active_index: usize,
    /// Called with the tab index when a tab is clicked.
    pub(super) on_select: Handler<'static, usize>,
}

/// Session tab bar showing open comparison sessions.
///
/// Rendered borderless on a single row to keep the chrome compact. When no
/// sessions are open the bar collapses to zero height, so the welcome
/// screen gets the full main view area. Clicking a tab activates it (the
/// keyboard counterparts are `Tab`/`Shift+Tab` and the arrow keys).
#[component]
pub(super) fn TabBar(
    props: &mut TabBarProps,
    mut hooks: Hooks,
) -> impl Into<AnyElement<'static>> {
    let theme = hooks.use_component_theme::<CocomoTheme>();

    if mouse_enabled() {
        let mut on_select = props.on_select.take();
        let titles = props.tab_titles.clone();
        hooks.use_pointer(PointerOptions::default(), move |ev| {
            let PointerKind::Click {
                button: MouseButton::Left,
                ..
            } = ev.kind
            else {
                return EventResult::Ignored;
            };
            let tab = ev
                .position
                .x
                .checked_sub(ev.area.x)
                .and_then(|x| tabs_hit_item(x, &titles, DIVIDER_WIDTH));
            match tab {
                Some(index) => {
                    on_select(index);
                    EventResult::Consumed
                }
                None => EventResult::Ignored,
            }
        });
    }

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
