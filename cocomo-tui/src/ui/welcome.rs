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
        prelude::Stylize,
        style::{Color, Style},
        text::Line,
    },
};

/// Props for [`WelcomeScreen`] — none; state is read from the global atom.
#[derive(Default, Props)]
pub(super) struct WelcomeScreenProps;

/// Welcome screen rendered when no sessions are open.
///
/// Displays the application name, a short description, and usage hints using
/// ratatui-kit layout components (`Center`, `Border`, `Text`) instead of
/// legacy Frame-based rendering.
#[component]
pub(super) fn WelcomeScreen(
    _props: &WelcomeScreenProps,
) -> impl Into<AnyElement<'static>> {
    let title_line = Line::from("cocomo \u{2014} COmpare, COpy & MOve")
        .bold()
        .cyan()
        .centered();
    let hint_line_1 =
        Line::from("Provide --left and --right to start a comparison.")
            .dark_gray()
            .centered();
    let hint_line_2 = Line::from("Press q to quit.").dark_gray().centered();

    element!(
        Center(width: Constraint::Length(58), height: Constraint::Length(16)) {
            Border(
                flex_direction: Direction::Vertical,
                justify_content: Flex::Center,
                border_style: Style::new().fg(Color::Gray),
                top_title: Line::from(" Welcome ").fg(Color::Yellow).bold().centered(),
            ) {
                Text(text: title_line)
                Text(text: Line::from(""))
                Text(text: hint_line_1)
                Text(text: Line::from(""))
                Text(text: hint_line_2)
            }
        }
    )
}
