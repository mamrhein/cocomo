// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Welcome screen rendered when no sessions are open.

use ratatui_kit::{
    prelude::*,
    ratatui::{
        layout::{Constraint, Direction, Flex},
        style::Styled,
        text::Line,
    },
};

use super::theme::CocomoTheme;

/// Welcome screen rendered when no sessions are open.
///
/// Displays the application name, a short description, and usage hints using
/// ratatui-kit layout components (`Center`, `Border`, `Text`) instead of
/// legacy Frame-based rendering.
#[component]
pub(super) fn WelcomeScreen(hooks: Hooks) -> impl Into<AnyElement<'static>> {
    let theme = hooks.use_component_theme::<CocomoTheme>();
    let title_line = Line::from("cocomo \u{2014} COmpare, COpy & MOve")
        .set_style(theme.brand)
        .centered();
    let hint_line_1 = Line::from(
        "Provide a left and a right directory to start a comparison.",
    )
    .set_style(theme.dim)
    .centered();
    let hint_line_2 = Line::from("Press q to quit.")
        .set_style(theme.dim)
        .centered();
    let welcome_title =
        Line::from(" Welcome ").set_style(theme.title).centered();

    element!(
        Center(width: Constraint::Length(58), height: Constraint::Length(16)) {
            Border(
                flex_direction: Direction::Vertical,
                justify_content: Flex::Center,
                gap: 1,
                border_style: theme.border,
                top_title: welcome_title,
            ) {
                Text(text: title_line)
                Text(text: hint_line_1)
                Text(text: hint_line_2)
            }
        }
    )
}
