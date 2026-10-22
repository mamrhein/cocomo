// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Shared styles used by more than one UI component.

use ratatui_kit::ratatui::style::{Color, Modifier, Style};

/// Border style for panels, tables, and bars.
pub(super) fn border_style() -> Style {
    Style::new().fg(Color::Gray)
}

/// Yellow bold style for titles and table headers.
pub(super) fn title_style() -> Style {
    Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD)
}

/// Reverse-video style for the selected table row.
pub(super) fn highlight_style() -> Style {
    Style::new()
        .bg(Color::DarkGray)
        .add_modifier(Modifier::REVERSED)
}
