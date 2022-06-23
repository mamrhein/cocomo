// ---------------------------------------------------------------------------
// Copyright:   (c) 2022 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

use tui::{
    backend::Backend,
    layout::{Constraint, Direction, Layout, Rect},
    Frame,
};

pub(crate) trait View<B: Backend> {
    fn want_layout(&self) -> Constraint;
    fn draw(&self, frame: &mut Frame<B>, area: Rect);
}

pub(crate) struct CompositeView<'a, B: Backend> {
    layout_direction: Direction,
    child_views: Vec<Box<dyn View<B> + 'a>>,
}

impl<'a, B: Backend> CompositeView<'a, B> {
    pub(crate) fn new(layout_direction: Direction) -> Self {
        Self {
            layout_direction,
            child_views: Vec::default(),
        }
    }
    pub(crate) fn add(mut self, view: Box<dyn View<B> + 'a>) -> Self {
        self.child_views.push(view);
        self
    }
}

impl<'a, B: Backend> View<B> for CompositeView<'a, B> {
    fn want_layout(&self) -> Constraint {
        // TODO: calculate from elems
        Constraint::Min(0)
    }

    fn draw(&self, frame: &mut Frame<B>, area: Rect) {
        let chunks = Layout::default()
            .direction(self.layout_direction.clone())
            .constraints(
                self.child_views
                    .iter()
                    .map(|v| v.want_layout())
                    .collect::<Vec<Constraint>>(),
            )
            .split(area);
        for (child, area) in self.child_views.iter().zip(chunks) {
            child.draw(frame, area);
        }
    }
}
