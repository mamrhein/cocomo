// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! # UI Module (`ui`)
//!
//! This module implements the `Widget` trait for the `App` struct,
//! defining how the entire user interface is rendered.

use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
    style::{Color, Style},
    widgets::{Block, Clear, Paragraph, Tabs, Widget},
};

use crate::app::{App, AppView};

impl Widget for &App {
    /// Renders the user interface widgets.
    fn render(self, area: Rect, buf: &mut Buffer) {
        // Create layout
        let vert_constraints = [
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
        ];
        let [menu_bar, tab_bar, main_view, key_bar] =
            Layout::vertical(vert_constraints).areas(area);

        // Render menu and key hints
        Paragraph::new("Menu").left_aligned().render(menu_bar, buf);
        Paragraph::new(
            "q: quit | x: close tab | Enter: open | Tab: switch | ↑/↓: \
             navigate | Home/End: top/bottom",
        )
        .left_aligned()
        .render(key_bar, buf);

        let titles: Vec<String> = self
            .views
            .iter()
            .map(|v| match v {
                AppView::Dir(dv) => {
                    dv.diff.left_dir.name().to_string_lossy().into_owned()
                }
                AppView::File(fv) => {
                    fv.left_item.name().to_string_lossy().into_owned()
                }
            })
            .collect();

        Tabs::new(titles)
            .select(self.active_view)
            .highlight_style(Style::default().fg(Color::Yellow).bold())
            .divider("|")
            .render(tab_bar, buf);

        // Render current view
        if let Some(view) = self.current_view() {
            match view {
                AppView::Dir(dir_view) => {
                    dir_view.render(main_view, buf);
                }
                AppView::File(file_view) => {
                    file_view.render(main_view, buf);
                }
            }
        }

        if self.show_quit_confirm {
            let area = centered_rect(40, 10, area);
            Clear.render(area, buf);
            let text = "Close last tab and quit? (y/n)";
            Paragraph::new(text)
                .centered()
                .block(Block::bordered())
                .render(area, buf);
        }
    }
}

/// helper function to create a centered rect using up certain % of the
/// available rect `r`
fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::vertical([
        Constraint::Percentage((100 - percent_y) / 2),
        Constraint::Percentage(percent_y),
        Constraint::Percentage((100 - percent_y) / 2),
    ])
    .split(r);

    Layout::horizontal([
        Constraint::Percentage((100 - percent_x) / 2),
        Constraint::Percentage(percent_x),
        Constraint::Percentage((100 - percent_x) / 2),
    ])
    .split(popup_layout[1])[1]
}
