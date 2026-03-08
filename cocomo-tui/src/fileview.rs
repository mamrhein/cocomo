// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------

use std::fs;

use cocomo_core::FSItem;
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
    widgets::{Block, Paragraph, Widget},
};

/// View for displaying side-by-side text file contents.
#[derive(Debug)]
pub struct FileView {
    /// Left item metadata.
    pub left_item: FSItem,
    /// Right item metadata.
    pub right_item: FSItem,
    /// Content of the left file.
    pub left_content: String,
    /// Content of the right file.
    pub right_content: String,
}

impl FileView {
    /// Creates a new `FileView` for two text files.
    pub async fn new(left_item: FSItem, right_item: FSItem) -> Self {
        let left_content =
            fs::read_to_string(left_item.path()).unwrap_or_default();
        let right_content =
            fs::read_to_string(right_item.path()).unwrap_or_default();
        Self {
            left_item,
            right_item,
            left_content,
            right_content,
        }
    }
}

impl Widget for &FileView {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let vert_constraints = [Constraint::Length(1), Constraint::Min(0)];
        let [header_area, content_area] =
            Layout::vertical(vert_constraints).areas(area);

        let horiz_constraints =
            [Constraint::Percentage(50), Constraint::Percentage(50)];
        let [left_header, right_header] =
            Layout::horizontal(horiz_constraints).areas(header_area);
        let [left_content_area, right_content_area] =
            Layout::horizontal(horiz_constraints).areas(content_area);

        Paragraph::new(self.left_item.path().to_string_lossy().as_ref())
            .render(left_header, buf);
        Paragraph::new(self.right_item.path().to_string_lossy().as_ref())
            .render(right_header, buf);

        Paragraph::new(self.left_content.as_str())
            .block(Block::bordered())
            .render(left_content_area, buf);
        Paragraph::new(self.right_content.as_str())
            .block(Block::bordered())
            .render(right_content_area, buf);
    }
}
