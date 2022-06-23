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
    layout::{Alignment, Constraint, Rect},
    style::{Color, Style},
    text::{Span, Spans},
    widgets::Paragraph,
    Frame,
};

use crate::view::View;

#[derive(Clone, Copy, Debug)]
pub(crate) struct CmdInfo<'a> {
    name: &'a str,
    key_hint: &'a str,
    is_visible: bool,
    is_active: bool,
}

impl<'a> CmdInfo<'a> {
    pub(crate) fn new(name: &'a str, key_hint: &'a str) -> Self {
        Self {
            name,
            key_hint,
            is_visible: true,
            is_active: true,
        }
    }

    pub(crate) fn hide(mut self) -> Self {
        self.is_visible = false;
        self
    }

    pub(crate) fn show(mut self) -> Self {
        self.is_visible = true;
        self
    }

    pub(crate) fn deactivate(mut self) -> Self {
        self.is_active = false;
        self
    }

    pub(crate) fn activate(mut self) -> Self {
        self.is_active = true;
        self
    }

    pub(crate) fn text(&self) -> String {
        format!("{} [{}]", self.name, self.key_hint)
    }
}

type CmdInfoList<'a> = Vec<CmdInfo<'a>>;

#[derive(Copy, Clone, Debug)]
enum CmdBarViewMode {
    Full,
    Compact,
}

#[derive(Clone, Debug)]
pub(crate) struct CmdBar<'a> {
    cmd_infos: CmdInfoList<'a>,
    view_mode: CmdBarViewMode,
}

impl<'a> CmdBar<'a> {
    pub(crate) fn new() -> Self {
        Self {
            cmd_infos: Vec::default(),
            view_mode: CmdBarViewMode::Compact,
        }
    }

    pub(crate) fn append_cmd(
        mut self,
        name: &'a str,
        key_hint: &'a str,
    ) -> Self {
        self.cmd_infos.push(CmdInfo::new(name, key_hint));
        self
    }
}

impl<'a, B: Backend> View<B> for &CmdBar<'a> {
    fn want_layout(&self) -> Constraint {
        Constraint::Min(1)
    }

    fn draw(&self, frame: &mut Frame<B>, area: Rect) {
        let cmd_bar = Paragraph::new(Spans::from(
            self.cmd_infos
                .iter()
                .flat_map(|c| {
                    [
                        Span::styled(
                            c.text(),
                            Style::default().bg(Color::LightYellow),
                        ),
                        Span::raw(" "),
                    ]
                })
                .collect::<Vec<Span>>(),
        ))
        .alignment(Alignment::Left);
        frame.render_widget(cmd_bar, area);
    }
}
