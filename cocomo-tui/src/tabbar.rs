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
    layout::{Constraint, Rect},
    style::{Color, Style},
    text::Spans,
    widgets::Tabs,
    Frame,
};

use crate::view::View;

pub(crate) struct TabBar<'a> {
    titles: Vec<&'a str>,
    curr_tab_idx: usize,
    n_lines: u8,
}

impl<'a> TabBar<'a> {
    pub(crate) fn new(titles: Vec<&'a str>, curr_tab_idx: usize) -> Self {
        Self {
            titles,
            curr_tab_idx,
            n_lines: 1,
        }
    }
}

impl<'a, B: Backend> View<B> for &TabBar<'a> {
    fn want_layout(&self) -> Constraint {
        Constraint::Min(1)
    }

    fn draw(&self, frame: &mut Frame<B>, area: Rect) {
        let titles = self
            .titles
            .iter()
            .enumerate()
            .map(|(idx, s)| {
                let name = format!("{} [{}]", &s, idx + 1);
                Spans::from(name)
            })
            .collect();
        let tabs = Tabs::new(titles)
            .select(self.curr_tab_idx)
            .highlight_style(Style::default().bg(Color::Gray))
            .divider("|");
        frame.render_widget(tabs, area);
    }
}
