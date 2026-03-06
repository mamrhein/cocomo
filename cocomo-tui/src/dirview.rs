// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

use cocomo_core::{DiffItemType, DiffSide, DirDiff};
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
    style::{Color, Style},
    widgets::{Block, Widget},
};

/// View for displaying directory comparison results.
#[derive(Debug)]
pub struct DirView<'a> {
    /// The comparison results.
    pub diff: &'a DirDiff,
    /// The index of the selected item.
    pub selected: usize,
}

impl Widget for DirView<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let horiz_constraints = [
            Constraint::Min(3),
            Constraint::Length(3),
            Constraint::Min(3),
        ];
        let [left_area, mid_area, right_area] =
            Layout::horizontal(horiz_constraints).areas(area);

        // Path headers
        let left_path = self.diff.left_dir.path().to_string_lossy();
        let right_path = self.diff.right_dir.path().to_string_lossy();

        let left_block = Block::bordered().title(left_path.as_ref());
        let right_block = Block::bordered().title(right_path.as_ref());

        let left_inner = left_block.inner(left_area);
        let right_inner = right_block.inner(right_area);

        left_block.render(left_area, buf);
        right_block.render(right_area, buf);

        let height = left_inner.height as usize;
        let total = self.diff.items.len();
        if total > 0 {
            // Simple scrolling logic
            let start = if self.selected >= height / 2 {
                (self.selected - height / 2).min(total.saturating_sub(height))
            } else {
                0
            };

            for (i, item) in
                self.diff.items.iter().skip(start).take(height).enumerate()
            {
                let y = left_inner.y + i as u16;
                let is_selected = (start + i) == self.selected;

                let mut style = Style::default();
                if is_selected {
                    style = style.bg(Color::Blue).fg(Color::White);
                }

                // Render left item name
                if let Some(left_item) = &item.left_item {
                    let name = left_item.name().to_string_lossy();
                    buf.set_stringn(
                        left_inner.x,
                        y,
                        &name,
                        left_inner.width as usize,
                        style,
                    );
                }

                // Render indicator
                let (indicator, ind_style) = match &item.diff_item_type {
                    DiffItemType::LeftOnly => {
                        (" + ", Style::default().fg(Color::Yellow))
                    }
                    DiffItemType::RightOnly => {
                        (" + ", Style::default().fg(Color::Yellow))
                    }
                    DiffItemType::Same { .. } => {
                        (" = ", Style::default().fg(Color::Green))
                    }
                    DiffItemType::Different { newer } => match newer {
                        Some(DiffSide::Left) => {
                            (" > ", Style::default().fg(Color::Red))
                        }
                        Some(DiffSide::Right) => {
                            (" < ", Style::default().fg(Color::Red))
                        }
                        None => (" ! ", Style::default().fg(Color::Red)),
                    },
                };
                buf.set_string(
                    mid_area.x,
                    y,
                    indicator,
                    if is_selected { style } else { ind_style },
                );

                // Render right item name
                if let Some(right_item) = &item.right_item {
                    let name = right_item.name().to_string_lossy();
                    buf.set_stringn(
                        right_inner.x,
                        y,
                        &name,
                        right_inner.width as usize,
                        style,
                    );
                }
            }
        }
    }
}
