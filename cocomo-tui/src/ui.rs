// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

/// Renders the widgets / UI.
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
    widgets::{Block, Paragraph, Widget},
};

use crate::app::App;

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
        let horiz_constraints = [
            Constraint::Min(3),
            Constraint::Length(1),
            Constraint::Min(3),
        ];
        let [left, mid, right] =
            Layout::horizontal(horiz_constraints).areas(main_view);
        // Create widgets
        let menu = Paragraph::new("Menu").left_aligned();
        let key_hints = Paragraph::new("q: quit").left_aligned();
        let left_path = self
            .cmp_items
            .left
            .as_ref()
            .map_or("<empty>", |item| item.path().to_str().unwrap());
        let left_view = Block::bordered().title(left_path);
        let indicator_column = Block::default();
        let right_path = self
            .cmp_items
            .right
            .as_ref()
            .map_or("<empty>", |item| item.path().to_str().unwrap());
        let right_view = Block::bordered().title(right_path);
        // Render widgets
        menu.render(menu_bar, buf);
        key_hints.render(key_bar, buf);
        left_view.render(left, buf);
        indicator_column.render(mid, buf);
        right_view.render(right, buf);
    }
}
