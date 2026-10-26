// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Shared bottom scaffolding: the one-line `StatusBar` and the multi-line
//! `HelpBar` used by both comparison views.
//!
//! Both bars are rendered borderless so they cost only the rows they
//! actually need; the panel above them already draws the separating
//! border, so a full box would only steal vertical space from the main
//! view area.
//!
//! The module also hosts [`tabs_hit_item`], the click mapping shared by
//! the menu and tab bars, which both render through ratatui's `Tabs`.

use ratatui_kit::{
    prelude::*,
    ratatui::{
        layout::Constraint,
        text::{Line, Text as RataText},
    },
};

/// Map an x coordinate local to a [`ratatui::widgets::Tabs`] row to the
/// index of the item it hits, `None` on a divider or past the last item.
///
/// Mirrors the widget's layout: every item renders as
/// `[pad-left][title][pad-right]` with `divider_width` cells between
/// items, so the clickable region of an item spans its title plus the two
/// padding cells.
pub(super) fn tabs_hit_item(
    x: u16,
    titles: &[Line<'static>],
    divider_width: u16,
) -> Option<usize> {
    let mut cursor = 0u16;
    for (i, title) in titles.iter().enumerate() {
        let end = cursor
            .saturating_add(title.width() as u16)
            .saturating_add(2);
        if x >= cursor && x < end {
            return Some(i);
        }
        cursor = end.saturating_add(divider_width);
    }
    None
}

/// Placeholder text of the command-mode (`:`) input, shared by both
/// comparison views.
pub(super) const COMMAND_PLACEHOLDER: &str =
    "Command (reload, sync, filter <pattern>, help)...";

/// Props for [`StatusBar`].
#[derive(Default, Props)]
pub(super) struct BarProps {
    /// The single content line of the bar.
    pub(super) line: Line<'static>,
}

/// One-line status bar at the bottom of a comparison view.
#[component]
pub(super) fn StatusBar(props: &BarProps) -> impl Into<AnyElement<'static>> {
    element!(
        View(height: Constraint::Length(1)) {
            Text(text: props.line.clone())
        }
    )
}

/// Props for [`HelpBar`].
#[derive(Default, Props)]
pub(super) struct HelpBarProps {
    /// The content lines of the bar; the bar grows to show them all.
    pub(super) lines: Vec<Line<'static>>,
}

/// Help bar at the very bottom of a comparison view.
///
/// Takes as many rows as its (pre-wrapped) content lines, so no help item
/// is cut off on narrow terminals. Only rendered when the user toggles it
/// on (`?`); when hidden the main view area gains the rows.
#[component]
pub(super) fn HelpBar(props: &HelpBarProps) -> impl Into<AnyElement<'static>> {
    element!(
        View(height: Constraint::Length(props.lines.len().max(1) as u16)) {
            Text(text: RataText::from(props.lines.clone()))
        }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn titles(names: &[&str]) -> Vec<Line<'static>> {
        names.iter().map(|n| Line::from(n.to_string())).collect()
    }

    #[test]
    fn tabs_hit_item_includes_the_padding_but_not_the_divider() {
        // Layout: `[x][Sessions][x] | [x][Help][x]` → item 0 spans 0..10,
        // the divider 10..13, item 1 spans 13..19.
        let items = titles(&["Sessions", "Help"]);
        assert_eq!(tabs_hit_item(0, &items, 3), Some(0));
        assert_eq!(tabs_hit_item(9, &items, 3), Some(0));
        assert_eq!(tabs_hit_item(10, &items, 3), None);
        assert_eq!(tabs_hit_item(12, &items, 3), None);
        assert_eq!(tabs_hit_item(13, &items, 3), Some(1));
        assert_eq!(tabs_hit_item(18, &items, 3), Some(1));
        assert_eq!(tabs_hit_item(19, &items, 3), None);
    }

    #[test]
    fn tabs_hit_item_is_empty_without_titles() {
        assert_eq!(tabs_hit_item(0, &[], 3), None);
    }
}
