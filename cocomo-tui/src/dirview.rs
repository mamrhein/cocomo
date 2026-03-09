// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

use std::cell::RefCell;

use cocomo_core::{DiffItemType, DirDiff};
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
    style::{Color, Style},
    widgets::{
        Cell, Paragraph, Row, StatefulWidget, Table, TableState, Widget,
    },
};

/// View for displaying directory comparison results.
#[derive(Debug)]
pub struct DirView {
    /// The comparison results.
    pub diff: DirDiff,
    /// The state of the table.
    pub table_state: RefCell<TableState>,
}

impl DirView {
    /// Creates a new `DirView` with the given comparison results.
    #[must_use]
    pub fn new(diff: DirDiff) -> Self {
        let mut table_state = TableState::default();
        if !diff.items.is_empty() {
            table_state.select(Some(0));
        }
        Self {
            diff,
            table_state: RefCell::new(table_state),
        }
    }

    /// Moves the selection up by one item.
    pub fn move_up(&mut self) {
        let mut table_state = self.table_state.borrow_mut();
        let i = match table_state.selected() {
            Some(i) => {
                if i == 0 {
                    0
                } else {
                    i - 1
                }
            }
            None => 0,
        };
        table_state.select(Some(i));
    }

    /// Moves the selection down by one item.
    pub fn move_down(&mut self) {
        let mut table_state = self.table_state.borrow_mut();
        let i = match table_state.selected() {
            Some(i) => {
                if i >= self.diff.items.len().saturating_sub(1) {
                    i
                } else {
                    i + 1
                }
            }
            None => 0,
        };
        table_state.select(Some(i));
    }

    /// Moves the selection to the first item.
    pub fn move_home(&mut self) {
        if !self.diff.items.is_empty() {
            self.table_state.borrow_mut().select(Some(0));
        }
    }

    /// Moves the selection to the last item.
    pub fn move_end(&mut self) {
        if !self.diff.items.is_empty() {
            let last = self.diff.items.len().saturating_sub(1);
            self.table_state.borrow_mut().select(Some(last));
        }
    }
}

impl Widget for &DirView {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let vert_constraints = [
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
        ];
        let [header_area, table_area, footer_area] =
            Layout::vertical(vert_constraints).areas(area);

        // Path headers
        let left_path = self.diff.left_dir.path().to_string_lossy();
        let right_path = self.diff.right_dir.path().to_string_lossy();

        let header_horiz_constraints = [
            Constraint::Min(10),    // Left Name
            Constraint::Length(10), // Left Size
            Constraint::Length(19), // Left Modified
            Constraint::Length(4),  // Indicator
            Constraint::Min(10),    // Right Name
            Constraint::Length(10), // Right Size
            Constraint::Length(19), // Right Modified
        ];
        let header_layout =
            Layout::horizontal(header_horiz_constraints).split(header_area);

        buf.set_string(
            header_layout[0].x,
            header_layout[0].y,
            left_path.as_ref(),
            Style::default().bold(),
        );
        buf.set_string(
            header_layout[4].x,
            header_layout[4].y,
            right_path.as_ref(),
            Style::default().bold(),
        );

        // Table
        let header_cells =
            ["Name", "Size", "Modified", "", "Name", "Size", "Modified"]
                .into_iter()
                .map(|h| Cell::from(h).style(Style::default().bold()));
        let header = Row::new(header_cells).height(1).bottom_margin(0);

        let rows = self.diff.items.iter().enumerate().map(|(i, item)| {
            let mut cells = Vec::new();

            // Left item
            if let Some(left) = &item.left_item {
                cells.push(Cell::from(
                    left.name().to_string_lossy().into_owned(),
                ));
                cells.push(Cell::from(
                    left.metadata()
                        .as_ref()
                        .map_or("".to_string(), |m| m.len().to_string()),
                ));
                cells.push(Cell::from(
                    left.modified().map_or("".to_string(), |t| {
                        t.format("%Y-%m-%d %H:%M:%S").to_string()
                    }),
                ));
            } else {
                cells.push(Cell::from(""));
                cells.push(Cell::from(""));
                cells.push(Cell::from(""));
            }

            // Diff type indicator
            let indicator = match &item.diff_item_type {
                DiffItemType::LeftOnly => "→",
                DiffItemType::RightOnly => "←",
                DiffItemType::Different { newer } => match newer {
                    Some(cocomo_core::DiffSide::Left) => "→",
                    Some(cocomo_core::DiffSide::Right) => "←",
                    None => "⇄",
                },
                DiffItemType::Same { by } => match by {
                    By::Metadata => "≟",
                    By::Content => "=",
                },
            };
            cells.push(Cell::from(indicator).style(Style::default().bold()));

            // Right item
            if let Some(right) = &item.right_item {
                cells.push(Cell::from(
                    right.name().to_string_lossy().into_owned(),
                ));
                cells.push(Cell::from(
                    right
                        .metadata()
                        .as_ref()
                        .map_or("".to_string(), |m| m.len().to_string()),
                ));
                cells.push(Cell::from(
                    right.modified().map_or("".to_string(), |t| {
                        t.format("%Y-%m-%d %H:%M:%S").to_string()
                    }),
                ));
            } else {
                cells.push(Cell::from(""));
                cells.push(Cell::from(""));
                cells.push(Cell::from(""));
            }

            let mut style = Style::default();
            if i % 2 != 0 {
                style = style.bg(Color::Rgb(30, 30, 30));
            }
            Row::new(cells).style(style)
        });

        let table = Table::new(
            rows,
            [
                Constraint::Min(10),
                Constraint::Length(10),
                Constraint::Length(19),
                Constraint::Length(4),
                Constraint::Min(10),
                Constraint::Length(10),
                Constraint::Length(19),
            ],
        )
        .header(header)
        .row_highlight_style(
            Style::default().bg(Color::Blue).fg(Color::White),
        );

        StatefulWidget::render(
            table,
            table_area,
            buf,
            &mut *self.table_state.borrow_mut(),
        );

        // Footer
        let count = self.diff.items.len();
        let footer_text = format!("{} items", count);
        Paragraph::new(footer_text).render(footer_area, buf);
    }
}
