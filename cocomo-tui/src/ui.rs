// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
    widgets::{Paragraph, Widget},
};

use crate::{app::App, dirview::DirView};

impl Widget for &App {
    /// Renders the user interface widgets.
    fn render(self, area: Rect, buf: &mut Buffer) {
        // Create layout
        let vert_constraints = [
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(1),
        ];
        let [menu_bar, main_view, key_bar] =
            Layout::vertical(vert_constraints).areas(area);

        // Render menu and key hints
        Paragraph::new("Menu").left_aligned().render(menu_bar, buf);
        Paragraph::new("q: quit | ↑/↓: navigate | Home/End: top/bottom")
            .left_aligned()
            .render(key_bar, buf);

        // Render items if available
        if let Some(diff) = &self.diff {
            DirView {
                diff,
                selected: self.selected,
            }
            .render(main_view, buf);
        }
    }
}
