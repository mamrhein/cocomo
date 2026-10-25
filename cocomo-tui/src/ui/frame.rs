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

use ratatui_kit::{
    prelude::*,
    ratatui::{
        layout::Constraint,
        text::{Line, Text as RataText},
    },
};

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
