// ---------------------------------------------------------------------------
// Copyright:   (c) 2022 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

use std::{borrow::BorrowMut, io, ops::Add};

use crossterm::{
    event,
    event::{Event, KeyCode},
};
use tui::{
    backend::Backend,
    layout::{Alignment, Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Span, Spans},
    widgets::{Block, Borders, Paragraph, Tabs},
    Frame, Terminal,
};

use crate::{session::Session, view::View};

pub(crate) struct App {
    sessions: Vec<Session>,
    curr_session_idx: usize,
}

impl App {
    pub(crate) fn new(session: Session) -> Self {
        Self {
            sessions: vec![session],
            curr_session_idx: 0,
        }
    }

    #[inline(always)]
    pub(crate) fn n_sessions(&self) -> usize {
        self.sessions.len()
    }

    #[inline(always)]
    pub(crate) fn curr_session(&self) -> &Session {
        &self.sessions[self.curr_session_idx]
    }

    #[inline(always)]
    pub(crate) fn curr_session_mut(&mut self) -> &mut Session {
        &mut self.sessions[self.curr_session_idx]
    }

    pub(crate) fn next_session(&mut self) -> bool {
        if self.n_sessions() == 1 {
            return false;
        }
        self.curr_session_idx = (self.curr_session_idx + 1) % self.n_sessions();
        true
    }

    pub(crate) fn prev_session(&mut self) -> bool {
        if self.n_sessions() == 1 {
            return false;
        }
        self.curr_session_idx = self
            .curr_session_idx
            .checked_sub(1)
            .unwrap_or(self.n_sessions() - 1);
        true
    }

    pub(crate) fn add_session(&mut self) {
        // TODO: call new session params popup
        let session = Session::new(
            self.n_sessions() + 1,
            Some("fake".to_string()),
            self.curr_session().left.clone(),
            self.curr_session().right.clone(),
        );
        let new_idx = self.n_sessions();
        self.sessions.push(session);
        self.curr_session_idx = new_idx;
    }

    pub(crate) fn run<B: Backend>(
        &mut self,
        terminal: &mut Terminal<B>,
    ) -> io::Result<()> {
        let mut redraw = true;
        loop {
            terminal.draw(|f| self.draw(f))?;
            if let Event::Key(key) = event::read()? {
                match key.code {
                    KeyCode::Char('q') => break,
                    KeyCode::Char('>') if self.n_sessions() > 1 => {
                        if self.next_session() {
                            redraw = true;
                        }
                    }
                    KeyCode::Char('<') if self.n_sessions() > 1 => {
                        if self.prev_session() {
                            redraw = true;
                        }
                    }
                    KeyCode::Char(c) if c.is_digit(10) => {
                        let id = c.to_digit(10).unwrap() as usize;
                        if id > 0 && id <= self.n_sessions() {
                            self.curr_session_idx = id - 1;
                            redraw = true;
                        }
                    }
                    KeyCode::Char('n') if self.n_sessions() < 9 => {
                        self.add_session();
                        redraw = true
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    }

    pub(crate) fn draw<B: Backend>(&mut self, frame: &mut Frame<B>) {
        let size = frame.size();
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints(
                [
                    Constraint::Length(1),
                    Constraint::Min(5),
                    Constraint::Length(1),
                ]
                .as_ref(),
            )
            .split(size);
        // TODO: replace by TabBar
        let titles = self
            .sessions
            .iter()
            .enumerate()
            .map(|(idx, s)| {
                let name = format!("{} [{}]", &s.name, idx + 1);
                Spans::from(name)
            })
            .collect();
        let tabs = Tabs::new(titles)
            .select(self.curr_session_idx)
            .highlight_style(Style::default().bg(Color::Gray))
            .divider("|");
        frame.render_widget(tabs, chunks[0]);
        // Session view
        let session = self.curr_session_mut();
        session.set_area(chunks[1]).draw::<B>(frame);
        // TODO: replace by CmdBar
        let cmd_bar = Paragraph::new(Spans::from(vec![
            Span::styled("Quit [q]", Style::default().bg(Color::LightYellow)),
            Span::raw(" "),
            Span::styled(
                format!(
                    "Tab [{}><]",
                    "123456789".split_at(self.n_sessions()).0
                ),
                Style::default().bg(Color::LightYellow),
            ),
            Span::raw(" "),
            Span::styled(
                "New session [n]",
                Style::default().bg(Color::LightYellow),
            ),
        ]))
        .alignment(Alignment::Left);
        frame.render_widget(cmd_bar, chunks[2]);
    }
}
