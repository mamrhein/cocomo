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

#[derive(Clone, Debug)]
pub(crate) struct Session<'a> {
    id: usize,
    pub(crate) name: &'a str,
    pub(crate) left: Rc<FSItem>,
    pub(crate) right: Rc<FSItem>,
}

impl<'a> Session<'a> {
    pub(crate) fn new(
        id: usize,
        name: Option<&'a str>,
        left: Rc<FSItem>,
        right: Rc<FSItem>,
    ) -> Self {
        assert_eq!(left.item_type, right.item_type);
        Self {
            id,
            name: name.unwrap_or(""),
            left,
            right,
        }
    }

    pub(crate) fn session_type(&self) -> ItemType {
        self.left.item_type
    }
}

impl<'a, B: Backend> View<B> for &Session<'a> {
    fn want_layout(&self) -> Constraint {
        Constraint::Min(3)
    }

    fn draw(&self, frame: &mut Frame<B>, area: Rect) {
        frame.render_widget(
            Block::default()
                .title(format!("view '{}'", self.id))
                .borders(Borders::ALL),
            area,
        );
    }
}
