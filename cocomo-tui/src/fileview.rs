// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! # File View Module (`fileview`)
//!
//! This module provides the `FileView` struct and its `Widget` implementation
//! for side-by-side comparison of text files.

use std::{cell, io};

use cocomo_core::{FSItem, FileDiff, LineDiffType};
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
    style::{Color, Style},
    text::Text,
    widgets::{Block, Cell, Row, StatefulWidget, Table, TableState, Widget},
};

use crate::view::NavigableView;

/// View for displaying side-by-side text file contents.
#[derive(Debug)]
pub struct FileView {
    /// The diff data between the two files.
    pub file_diff: FileDiff,
    /// The state of the table.
    pub table_state: cell::RefCell<TableState>,
    /// The index of the currently selected chunk.
    pub current_chunk: usize,
}

impl FileView {
    /// Creates a new `FileView` for two text files.
    pub async fn new(
        left_item: &Option<FSItem>,
        right_item: &Option<FSItem>,
    ) -> io::Result<Self> {
        let file_diff = FileDiff::new(left_item, right_item).await?;
        let mut table_state = TableState::default();
        if !file_diff.chunks.is_empty() {
            table_state.select(Some(0));
        }
        Ok(Self {
            file_diff,
            table_state: cell::RefCell::new(table_state),
            current_chunk: 0,
        })
    }

    fn first_row_of_chunk(&self, chunk_idx: usize) -> usize {
        self.file_diff
            .chunks
            .iter()
            .take(chunk_idx)
            .map(|c| c.left_lines.len())
            .sum()
    }
}

impl NavigableView for FileView {
    /// Makes the previous chunk the current chunk.
    fn prev(&mut self) {
        if self.current_chunk > 0 {
            self.current_chunk -= 1;
        }
        let row_idx = self.first_row_of_chunk(self.current_chunk);
        self.table_state.borrow_mut().select(Some(row_idx));
    }

    /// Makes the next chunk the current chunk.
    fn next(&mut self) {
        if !self.file_diff.chunks.is_empty()
            && self.current_chunk
                < self.file_diff.chunks.len().saturating_sub(1)
        {
            self.current_chunk += 1;
        }
        let row_idx = self.first_row_of_chunk(self.current_chunk);
        self.table_state.borrow_mut().select(Some(row_idx));
    }

    /// Makes the first chunk the current chunk.
    fn home(&mut self) {
        self.current_chunk = 0;
        self.table_state.borrow_mut().select(Some(0));
    }

    /// Makes the last chunk the current chunk.
    fn end(&mut self) {
        if !self.file_diff.chunks.is_empty() {
            self.current_chunk = self.file_diff.chunks.len().saturating_sub(1);
            let row_idx = self.first_row_of_chunk(self.current_chunk);
            self.table_state.borrow_mut().select(Some(row_idx));
        }
    }
}

fn indicator<'a>(dt: LineDiffType) -> Text<'a> {
    let (char, color) = match dt {
        LineDiffType::Removed => ("-", Color::Red),
        LineDiffType::Added => ("+", Color::Green),
        LineDiffType::Unchanged => ("=", Color::White),
        LineDiffType::Changed => ("⇄", Color::Yellow),
    };
    Text::from(char)
        .style(Style::default().fg(color).bold())
        .centered()
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
            String::new()
        } else {
            self.file_diff
                .left_file
                .path()
                .to_string_lossy()
                .to_string()
        };
        let right_path = if self.file_diff.right_file.name().is_empty() {
            String::new()
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

            if chunk_idx == self.current_chunk {
                chunk_style = chunk_style.fg(Color::Cyan).bold();
            }

            for (left, right) in
                chunk.left_lines.iter().zip(&chunk.right_lines)
            {
                let cells = vec![
                    Cell::from(
                        left.line_number
                            .map_or(String::new(), |n| n.to_string()),
                    ),
                    Cell::from(left.content.as_str()),
                    Cell::from(indicator(chunk.diff_type)),
                    Cell::from(
                        right
                            .line_number
                            .map_or(String::new(), |n| n.to_string()),
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
