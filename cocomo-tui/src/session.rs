// ---------------------------------------------------------------------------
// Copyright:   (c) 2022 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

use std::rc::Rc;

use cocomo_core::{FSItem, ItemType};
use tui::{
    backend::Backend,
    layout::{Constraint, Rect},
    widgets::{Block, Borders},
    Frame,
};

use crate::view::View;

pub(crate) struct Session {
    id: usize,
    area: Rect,
    pub(crate) name: String,
    pub(crate) left: Rc<FSItem>,
    pub(crate) right: Rc<FSItem>,
}

impl Session {
    pub(crate) fn new(
        id: usize,
        name: Option<String>,
        left: Rc<FSItem>,
        right: Rc<FSItem>,
    ) -> Self {
        assert_eq!(left.item_type, right.item_type);
        Self {
            id,
            area: Rect::default(),
            name: name.unwrap_or("".to_string()),
            left,
            right,
        }
    }

    pub(crate) fn session_type(&self) -> ItemType {
        self.left.item_type
    }
}

impl View for Session {
    fn area(&self) -> Rect {
        self.area
    }

    fn set_area(&mut self, area: Rect) -> &Self {
        self.area = area;
        self
    }

    fn want_layout(&self) -> Constraint {
        Constraint::Min(3)
    }

    fn draw<B: Backend>(&self, frame: &mut Frame<B>) {
        frame.render_widget(
            Block::default()
                .title(format!("view '{}'", self.id))
                .borders(Borders::ALL),
            self.area,
        );
    }
}
