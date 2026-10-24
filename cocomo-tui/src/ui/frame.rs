// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Shared one-line scaffolding: the `StatusBar` and `HelpBar` used by both
//! comparison views.
//!
//! Both bars are rendered borderless so each costs a single row; the panel
//! above them already draws the separating border, so a full box would only
//! steal vertical space from the main view area.

use ratatui_kit::{
    prelude::*,
    ratatui::{layout::Constraint, text::Line},
};

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
        View(height: Constraint::Length(1)) {
            Text(text: props.line.clone())
        }
    )
}

/// One-line help bar at the very bottom of a comparison view.
///
/// Only rendered when the user toggles it on (`?`); when hidden the main
/// view area gains the row.
#[component]
pub(super) fn HelpBar(props: &BarProps) -> impl Into<AnyElement<'static>> {
    element!(
        View(height: Constraint::Length(1)) {
            Text(text: props.line.clone())
        }
    )
}
