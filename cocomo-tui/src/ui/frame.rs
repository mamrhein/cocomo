// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Shared bordered scaffolding: the header `Frame` and the one-line
//! `StatusBar` / `HelpBar` used by both comparison views.

use ratatui_kit::{
    prelude::*,
    ratatui::{
        layout::{Constraint, Direction},
        style::Styled,
        text::Line,
    },
};

use super::theme::CocomoTheme;

/// Props for [`Frame`].
#[derive(Default, Props)]
pub(super) struct FrameProps {
    /// Panel title rendered centered in the top border, styled with the
    /// theme's title style.
    pub(super) title: Option<Line<'static>>,
    /// Height constraint for the panel.
    pub(super) height: Constraint,
    /// Content lines rendered inside the panel, one `Text` per line.
    pub(super) lines: Vec<Line<'static>>,
}

/// Bordered panel with an optional centered title and content lines.
///
/// The shared header/status/help scaffolding of the comparison views
/// (F16): the border and title styles come from [`CocomoTheme`], so a view
/// only supplies geometry and content.
#[component]
pub(super) fn Frame(
    props: &FrameProps,
    hooks: Hooks,
) -> impl Into<AnyElement<'static>> {
    let theme = hooks.use_component_theme::<CocomoTheme>();
    let top_title = props
        .title
        .clone()
        .map(|title| title.set_style(theme.title).centered());
    let lines = props.lines.clone();

    element!(
        Border(
            height: props.height,
            flex_direction: Direction::Vertical,
            border_style: theme.border,
            top_title: top_title,
        ) {
            for (i, line) in lines.into_iter().enumerate() {
                Text(key: i, text: line)
            }
        }
    )
}

/// Props for [`StatusBar`] and [`HelpBar`].
#[derive(Default, Props)]
pub(super) struct BarProps {
    /// The single content line of the bar.
    pub(super) line: Line<'static>,
}

/// One-line status bar at the bottom of a comparison view.
#[component]
pub(super) fn StatusBar(props: &BarProps) -> impl Into<AnyElement<'static>> {
    element!(
        Frame(
            height: Constraint::Length(3),
            lines: vec![props.line.clone()],
        )
    )
}

/// One-line help bar at the very bottom of a comparison view.
#[component]
pub(super) fn HelpBar(props: &BarProps) -> impl Into<AnyElement<'static>> {
    element!(
        Frame(
            height: Constraint::Length(3),
            lines: vec![props.line.clone()],
        )
    )
}
