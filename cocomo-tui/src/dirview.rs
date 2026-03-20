// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! # Directory View Module (`dirview`)
//!
//! This module provides the `DirView` struct and its `Widget` implementation
//! for rendering directory comparison results in a table.

use std::{cell, io, path};

use cocomo_core::{
    By,
    DiffItem,
    DiffItemType,
    DirDiff,
    FSItem,
    copy_item,
    delete_item,
    move_item,
    // rename_item,
};
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
    style::{Color, Style},
    text::Text,
    widgets::{
        Cell, Paragraph, Row, StatefulWidget, Table, TableState, Widget,
    },
};

use crate::{appevent::AppEvent, view::NavigableView};

/// Map DirDiffType to indicator text
fn indicator<'a>(t: DiffItemType) -> Text<'a> {
    let (char, color) = match t {
        DiffItemType::LeftOnly => ("→", Color::Green),
        DiffItemType::RightOnly => ("←", Color::Green),
        DiffItemType::Different { newer } => match newer {
            Some(cocomo_core::DiffSide::Left) => ("→", Color::Yellow),
            Some(cocomo_core::DiffSide::Right) => ("←", Color::Yellow),
            None => ("⇄", Color::Yellow),
        },
        DiffItemType::Same { by } => match by {
            By::Metadata => ("≟", Color::White),
            By::Content => ("=", Color::White),
        },
    };
    Text::from(char)
        .style(Style::default().fg(color).bold())
        .centered()
}

/// View for displaying directory comparison results.
#[derive(Debug)]
pub struct DirView {
    /// The comparison results.
    pub diff: DirDiff,
    /// The state of the table.
    pub table_state: cell::RefCell<TableState>,
}

impl DirView {
    /// Creates a new `DirView` from the given file system items.
    pub async fn new(
        left_item: &Option<FSItem>,
        right_item: &Option<FSItem>,
    ) -> io::Result<Self> {
        let diff =
            DirDiff::new(left_item, right_item).await?;
        let mut table_state = TableState::default();
        if !diff.items.is_empty() {
            table_state.select(Some(0));
        }
        Ok(Self {
            diff,
            table_state: cell::RefCell::new(table_state),
        })
    }

    pub fn current_item(&self) -> Option<&DiffItem> {
        let table_state = self.table_state.borrow();
        let i = table_state.selected()?;
        Some(&self.diff.items[i])
    }

    pub(crate) async fn handle_app_event(&mut self, app_event: AppEvent) -> color_eyre::Result<()> {
        match app_event {
            AppEvent::Copy => {
                if let Some(item) = self.current_item() {
                    let r_dir = &self.diff.right_dir;
                    let l_dir = &self.diff.left_dir;
                    if let Some(l) = &item.left_item
                        && r_dir.path().exists()
                    {
                        let dst = r_dir.path().join(l.name());
                        copy_item(l, &dst).await?;
                    } else if let Some(r) = &item.right_item
                        && l_dir.path().exists()
                    {
                        let dst = l_dir.path().join(r.name());
                        copy_item(r, &dst).await?;
                    }
                }
            }
            AppEvent::Move => {
                if let Some(item) = self.current_item() {
                    let r_dir = &self.diff.right_dir;
                    let l_dir = &self.diff.left_dir;
                    if let Some(l) = &item.left_item
                        && r_dir.path().exists()
                    {
                        let dst = r_dir.path().join(l.name());
                        move_item(l, &dst).await?;
                    } else if let Some(r) = &item.right_item
                        && l_dir.path().exists()
                    {
                        let dst = l_dir.path().join(r.name());
                        move_item(r, &dst).await?;
                    }
                }
            }
            AppEvent::Delete => {
                if let Some(item) = self.current_item() {
                    if let Some(l) = &item.left_item {
                        delete_item(l).await?;
                    } else if let Some(r) = &item.right_item {
                        delete_item(r).await?;
                    }
                }
            }
            // AppEvent::Rename => {
            // let _ = rename_item(&item, &new_name).await;
            // }
            AppEvent::Refresh => {
                let left = self
                    .diff
                    .left_dir
                    .path()
                    .exists()
                    .then_some(self.diff.left_dir.clone());
                let right = self
                    .diff
                    .right_dir
                    .path()
                    .exists()
                    .then_some(self.diff.right_dir.clone());
                if let Ok(new_diff) = DirDiff::new(&left, &right).await {
                    self.diff = new_diff;
                }
            }
            _ => {} // ignore it (TODO: handle it)
        }
        Ok(())
    }
}

impl NavigableView for DirView {
    /// Makes the previous item the current item.
    fn prev(&mut self) {
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

    /// Makes the next item the current item.
    fn next(&mut self) {
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

    /// Makes the first item the current item.
    fn home(&mut self) {
        if !self.diff.items.is_empty() {
            self.table_state.borrow_mut().select(Some(0));
        }
    }

    /// Makes the last item the current item.
    fn end(&mut self) {
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
        let left_path = if self.diff.left_dir.name().is_empty() {
            String::new()
        } else {
            self.diff.left_dir.path().to_string_lossy().to_string()
        };
        let right_path = if self.diff.right_dir.name().is_empty() {
            String::new()
        } else {
            self.diff.right_dir.path().to_string_lossy().to_string()
        };

        let horiz_constraints = [
            Constraint::Min(10),    // Left Name
            Constraint::Length(10), // Left Size
            Constraint::Length(19), // Left Modified
            Constraint::Length(3),  // Indicator
            Constraint::Min(10),    // Right Name
            Constraint::Length(10), // Right Size
            Constraint::Length(19), // Right Modified
        ];
        let header_layout =
            Layout::horizontal(horiz_constraints).split(header_area);

        buf.set_string(
            header_layout[0].x,
            header_layout[0].y,
            &left_path,
            Style::default().bold(),
        );
        buf.set_string(
            header_layout[4].x + 1,
            header_layout[4].y,
            &right_path,
            Style::default().bold(),
        );

        // Table
        let header_cells =
            ["Name", "Size", "Modified", "", "Name", "Size", "Modified"]
                .into_iter()
                .map(|h| Cell::from(h).style(Style::default().bold()));
        let header = Row::new(header_cells)
            .height(1)
            .style(Style::default().bg(Color::Rgb(70, 70, 70)));

        let rows = self.diff.items.iter().enumerate().map(|(i, item)| {
            let mut cells = Vec::new();

            // Left item
            if let Some(left) = &item.left_item {
                let mut name = left.name().to_string_lossy();
                if left.is_dir() {
                    name += path::MAIN_SEPARATOR_STR;
                };
                cells.push(Cell::from(name.into_owned()));
                cells.push(Cell::from(
                    left.metadata()
                        .as_ref()
                        .map_or(String::new(), |m| m.len().to_string()),
                ));
                cells.push(Cell::from(
                    left.modified().map_or(String::new(), |t| {
                        t.format("%Y-%m-%d %H:%M:%S").to_string()
                    }),
                ));
            } else {
                cells.push(Cell::from(""));
                cells.push(Cell::from(""));
                cells.push(Cell::from(""));
            }

            // Diff type indicator
            cells.push(Cell::from(indicator(item.diff_item_type)));

            // Right item
            if let Some(right) = &item.right_item {
                let mut name = right.name().to_string_lossy();
                if right.is_dir() {
                    name += path::MAIN_SEPARATOR_STR;
                };
                cells.push(Cell::from(name.into_owned()));
                cells.push(Cell::from(
                    right
                        .metadata()
                        .as_ref()
                        .map_or(String::new(), |m| m.len().to_string()),
                ));
                cells.push(Cell::from(
                    right.modified().map_or(String::new(), |t| {
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
                style = style.bg(Color::Rgb(40, 40, 40));
            }
            Row::new(cells).style(style)
        });

        let table = Table::new(rows, horiz_constraints)
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
