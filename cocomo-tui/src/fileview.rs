// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------

//! # File View Module (`fileview`)
//!
//! This module provides the `FileView` struct and its `Widget` implementation
//! for side-by-side comparison of text files.

use std::cell::RefCell;

use cocomo_core::{FSItem, FileDiff, LineDiffType};
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
    style::{Color, Style},
    widgets::{Block, Cell, Row, StatefulWidget, Table, TableState, Widget},
};

/// View for displaying side-by-side text file contents.
#[derive(Debug)]
pub struct FileView {
    /// The diff data between the two files.
    pub file_diff: FileDiff,
    /// The state of the table.
    pub table_state: RefCell<TableState>,
    /// The index of the currently selected chunk.
    pub selected_chunk: usize,
}

impl FileView {
    /// Creates a new `FileView` for two text files.
    pub async fn new(
        left_item: Option<FSItem>,
        right_item: Option<FSItem>,
    ) -> Self {
        let file_diff = FileDiff::new(left_item, right_item)
            .await
            .expect("Failed to read files for diffing");
        let mut table_state = TableState::default();
        if !file_diff.chunks.is_empty() {
            table_state.select(Some(0));
        }
        Self {
            file_diff,
            table_state: RefCell::new(table_state),
            selected_chunk: 0,
        }
    }

    fn first_row_of_chunk(&self, chunk_idx: usize) -> usize {
        self.file_diff
            .chunks
            .iter()
            .take(chunk_idx)
            .map(|c| c.left_lines.len())
            .sum()
    }

    /// Moves the selection up by one chunk.
    pub fn move_up(&mut self) {
        if self.selected_chunk > 0 {
            self.selected_chunk -= 1;
        }
        let row_idx = self.first_row_of_chunk(self.selected_chunk);
        self.table_state.borrow_mut().select(Some(row_idx));
    }

    /// Moves the selection down by one chunk.
    pub fn move_down(&mut self) {
        if !self.file_diff.chunks.is_empty() {
            if self.selected_chunk
                < self.file_diff.chunks.len().saturating_sub(1)
            {
                self.selected_chunk += 1;
            }
        }
        let row_idx = self.first_row_of_chunk(self.selected_chunk);
        self.table_state.borrow_mut().select(Some(row_idx));
    }

    /// Moves the selection to the first chunk.
    pub fn move_home(&mut self) {
        self.selected_chunk = 0;
        self.table_state.borrow_mut().select(Some(0));
    }

    /// Moves the selection to the last chunk.
    pub fn move_end(&mut self) {
        if !self.file_diff.chunks.is_empty() {
            self.selected_chunk =
                self.file_diff.chunks.len().saturating_sub(1);
            let row_idx = self.first_row_of_chunk(self.selected_chunk);
            self.table_state.borrow_mut().select(Some(row_idx));
        }
    }
}

fn map_diff_type(dt: LineDiffType) -> &'static str {
    match dt {
        LineDiffType::Removed => "-",
        LineDiffType::Added => "+",
        LineDiffType::Unchanged => " ",
        LineDiffType::Changed => "M",
    }
}

impl Widget for &FileView {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let vert_constraints = [Constraint::Length(1), Constraint::Min(0)];
        let [header_area, content_area] =
            Layout::vertical(vert_constraints).areas(area);

        let horiz_constraints = [
            Constraint::Length(5), // Left Line No
            Constraint::Min(10),   // Left Content
            Constraint::Length(3), // Indicator
            Constraint::Length(5), // Right Line No
            Constraint::Min(10),   // Right Content
        ];
        let header_layout =
            Layout::horizontal(horiz_constraints).split(header_area);

        let left_path = if self.file_diff.left_file.name().is_empty() {
            "".to_string()
        } else {
            self.file_diff
                .left_file
                .path()
                .to_string_lossy()
                .to_string()
        };
        let right_path = if self.file_diff.right_file.name().is_empty() {
            "".to_string()
        } else {
            self.file_diff
                .right_file
                .path()
                .to_string_lossy()
                .to_string()
        };

        buf.set_string(
            header_layout[0].x,
            header_layout[0].y,
            &left_path,
            Style::default().bold(),
        );
        buf.set_string(
            header_layout[3].x,
            header_layout[3].y,
            &right_path,
            Style::default().bold(),
        );

        let mut rows = Vec::new();

        for (chunk_idx, chunk) in self.file_diff.chunks.iter().enumerate() {
            let mut chunk_style = match chunk.diff_type {
                LineDiffType::Removed => {
                    Style::default().bg(Color::Rgb(80, 0, 0))
                }
                LineDiffType::Added => {
                    Style::default().bg(Color::Rgb(0, 80, 0))
                }
                LineDiffType::Changed => {
                    Style::default().bg(Color::Rgb(80, 80, 0))
                }
                LineDiffType::Unchanged => Style::default(),
            };

            if chunk_idx == self.selected_chunk {
                chunk_style = chunk_style.fg(Color::Cyan).bold();
            }

            let indicator = map_diff_type(chunk.diff_type);

            for (left, right) in
                chunk.left_lines.iter().zip(&chunk.right_lines)
            {
                let cells = vec![
                    Cell::from(
                        left.line_number
                            .map_or("".to_string(), |n| n.to_string()),
                    ),
                    Cell::from(left.content.as_str()),
                    Cell::from(indicator),
                    Cell::from(
                        right
                            .line_number
                            .map_or("".to_string(), |n| n.to_string()),
                    ),
                    Cell::from(right.content.as_str()),
                ];
                rows.push(Row::new(cells).style(chunk_style));
            }
        }

        let table =
            Table::new(rows, horiz_constraints).block(Block::bordered());

        StatefulWidget::render(
            table,
            content_area,
            buf,
            &mut *self.table_state.borrow_mut(),
        );
    }
}
