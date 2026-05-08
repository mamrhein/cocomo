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

use core::cell::{Ref, RefMut};
use std::{cell, io, path};

use cocomo_core::{
    By,
    DiffItem,
    DiffItemType,
    DiffSide,
    DirDiff,
    FSItem,
    copy_item,
    delete_item,
    move_item, // rename_item,
};
use futures::executor::block_on;
use ratatui::{
    buffer::Buffer,
    crossterm::event::KeyCode,
    layout::{Constraint, Layout, Rect},
    style::{Color, Style},
    text::Text,
    widgets::{
        Cell, Paragraph, Row, StatefulWidget, Table, TableState, Widget,
        WidgetRef,
    },
};

use crate::{
    event::{Event, OpEvent},
    keymap::{
        GroupedKeyMap, HasKeyMap, KeyHint, KeyMapItem, KeyMapper,
        SingleKeyMap,
    },
    view::{NAV_KEYMAP_ITEMS, TableView, TableViewState, View},
};

/// Key map items for ops keymap.
#[rustfmt::skip]
pub(crate) const OP_KEYMAP_ITEMS: [KeyMapItem; 4] = [
    KeyMapItem::new(
        KeyCode::Char('c'),
        None,
        "Copy",
        true,
        Event::Op(OpEvent::Copy),
    ),
    KeyMapItem::new(
        KeyCode::Char('m'),
        None,
        "Move",
        true,
        Event::Op(OpEvent::Move),
    ),
    KeyMapItem::new(
        KeyCode::Char('d'),
        None,
        "Delete",
        true,
        Event::Op(OpEvent::Delete),
    ),
    KeyMapItem::new(
        KeyCode::Char('r'),
        None,
        "Rename",
        // TODO: enable when op rename is implemented
        false,
        Event::Op(OpEvent::Rename),
    ),
];

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
    /// View level key maps
    keymap: GroupedKeyMap<2>,
    /// The comparison results.
    diff: DirDiff,
    /// The state of the table.
    table_state: cell::RefCell<TableState>,
}

impl DirView {
    /// Creates a new `DirView` from the given file system items.
    pub async fn new(
        left_item: &Option<FSItem>,
        right_item: &Option<FSItem>,
    ) -> io::Result<Self> {
        let diff = DirDiff::new(left_item, right_item).await?;
        let mut table_state = TableState::default();
        if !diff.items.is_empty() {
            table_state.select(Some(0));
        }
        Ok(Self {
            keymap: GroupedKeyMap::new([
                SingleKeyMap::from(NAV_KEYMAP_ITEMS.as_slice()),
                SingleKeyMap::from(OP_KEYMAP_ITEMS.as_slice()),
            ]),
            diff,
            table_state: cell::RefCell::new(table_state),
        })
    }

    pub(crate) async fn handle_op_event(
        &mut self,
        op_event: OpEvent,
    ) -> color_eyre::Result<()> {
        let left_dir = &self.diff.left_dir;
        let right_dir = &self.diff.right_dir;
        match op_event {
            OpEvent::Copy => {
                if let Some(item) = self.current_diff_item()
                    && left_dir.is_some()
                    && right_dir.is_some()
                {
                    let (src, dst) = match item.diff_item_type {
                        DiffItemType::LeftOnly
                        | DiffItemType::Different {
                            newer: Some(DiffSide::Left),
                        }
                        | DiffItemType::Different { newer: None } => (
                            item.left_item.as_ref().unwrap(),
                            right_dir.as_ref().unwrap(),
                        ),
                        DiffItemType::RightOnly
                        | DiffItemType::Different {
                            newer: Some(DiffSide::Right),
                        } => (
                            item.right_item.as_ref().unwrap(),
                            left_dir.as_ref().unwrap(),
                        ),
                        DiffItemType::Same { by } => {
                            if by == By::Content {
                                return Ok(());
                            };
                            (
                                item.left_item.as_ref().unwrap(),
                                right_dir.as_ref().unwrap(),
                            )
                        }
                    };
                    copy_item(src, dst.path()).await?;
                    self.diff.refresh().await?;
                }
            }
            OpEvent::Move => {
                if let Some(item) = self.current_diff_item()
                    && left_dir.is_some()
                    && right_dir.is_some()
                {
                    let (src, dst) = match item.diff_item_type {
                        DiffItemType::LeftOnly
                        | DiffItemType::Different {
                            newer: Some(DiffSide::Left),
                        }
                        | DiffItemType::Different { newer: None } => (
                            item.left_item.as_ref().unwrap(),
                            right_dir.as_ref().unwrap(),
                        ),
                        DiffItemType::RightOnly
                        | DiffItemType::Different {
                            newer: Some(DiffSide::Right),
                        } => (
                            item.right_item.as_ref().unwrap(),
                            left_dir.as_ref().unwrap(),
                        ),
                        DiffItemType::Same { by } => {
                            if by == By::Content {
                                delete_item(item.left_item.as_ref().unwrap())
                                    .await?;
                                return Ok(());
                            }
                            (
                                item.left_item.as_ref().unwrap(),
                                right_dir.as_ref().unwrap(),
                            )
                        }
                    };
                    move_item(src, dst.path()).await?;
                    self.diff.refresh().await?;
                }
            }
            OpEvent::Delete => {
                if let Some(item) = self.current_diff_item() {
                    let target = match item.diff_item_type {
                        DiffItemType::LeftOnly
                        | DiffItemType::Different { newer: None }
                        | DiffItemType::Different {
                            newer: Some(DiffSide::Right),
                        }
                        | DiffItemType::Same { .. } => {
                            item.left_item.as_ref().unwrap()
                        }
                        DiffItemType::RightOnly
                        | DiffItemType::Different {
                            newer: Some(DiffSide::Left),
                        } => item.right_item.as_ref().unwrap(),
                    };
                    delete_item(target).await?;
                    self.diff.refresh().await?;
                }
            }
            // OpEvent::Rename => {
            // let _ = rename_item(&item, &new_name).await;
            // }
            OpEvent::Reload => {
                self.diff.refresh().await?;
            }
            _ => {} // ignore it (TODO: handle it)
        }
        Ok(())
    }
}

impl HasKeyMap for DirView {
    type T = GroupedKeyMap<2>;
    fn keymap(&self) -> &Self::T {
        &self.keymap
    }
}

impl KeyHint for DirView {
    #[inline(always)]
    fn key_hint(&self) -> Text<'_> {
        Text::from(&self.keymap)
    }
}

impl KeyMapper for DirView {
    #[inline(always)]
    fn keymapper(&self) -> &dyn KeyMapper {
        &self.keymap
    }
}

impl View for DirView {
    fn title(&self) -> String {
        self.diff.name().to_string_lossy().into_owned()
    }

    fn is_dir_view(&self) -> bool {
        true
    }

    fn current_diff_item(&self) -> Option<&DiffItem> {
        let table_state = self.table_state.borrow();
        let i = table_state.selected()?;
        Some(&self.diff.items[i])
    }

    fn handle_op_event(
        &mut self,
        op_event: OpEvent,
    ) -> color_eyre::Result<()> {
        block_on(self.handle_op_event(op_event))
    }

    fn is_file_view(&self) -> bool {
        // There will only be one directory view but several file views.
        true
    }
}

impl TableViewState for DirView {
    #[inline(always)]
    fn n_items(&self) -> usize {
        self.diff.items.len()
    }

    #[inline(always)]
    fn table_state(&self) -> Ref<'_, TableState> {
        self.table_state.borrow()
    }

    #[inline(always)]
    fn table_state_mut(&mut self) -> RefMut<'_, TableState> {
        self.table_state.borrow_mut()
    }
}

impl TableView for DirView {
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

impl WidgetRef for DirView {
    fn render_ref(&self, area: Rect, buf: &mut Buffer) {
        let vert_constraints = [
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
        ];
        let [header_area, table_area, footer_area] =
            Layout::vertical(vert_constraints).areas(area);

        // Path headers
        let left_path = self
            .diff
            .left_dir
            .as_ref()
            .unwrap_or(&FSItem::default())
            .path()
            .to_string_lossy()
            .to_string();
        let right_path = self
            .diff
            .right_dir
            .as_ref()
            .unwrap_or(&FSItem::default())
            .path()
            .to_string_lossy()
            .to_string();

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
