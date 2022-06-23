// ---------------------------------------------------------------------------
// Copyright:   (c) 2022 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

use std::io;

use crossterm::{
    event,
    event::{Event, KeyCode},
};
use tui::{backend::Backend, layout::Direction, Frame, Terminal};

use crate::{
    cmdbar::CmdBar,
    session::Session,
    tabbar::TabBar,
    view::{CompositeView, View},
};

pub(crate) struct App<'a> {
    sessions: Vec<Session<'a>>,
    curr_session_idx: usize,
}

impl<'a> App<'a> {
    pub(crate) fn new(session: Session<'a>) -> Self {
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
    pub(crate) fn curr_session(&self) -> &'a Session {
        &self.sessions[self.curr_session_idx]
    }

    #[inline(always)]
    pub(crate) fn curr_session_mut(&mut self) -> &'a mut Session {
        &mut self.sessions[self.curr_session_idx]
    }

    pub(crate) fn activate_next_session(&mut self) {
        self.curr_session_idx = (self.curr_session_idx + 1) % self.n_sessions();
    }

    pub(crate) fn activate_prev_session(&mut self) {
        self.curr_session_idx = self
            .curr_session_idx
            .checked_sub(1)
            .unwrap_or(self.n_sessions() - 1);
    }

    pub(crate) fn add_session(&mut self) {
        // TODO: call new session params popup
        let session = Session::new(
            self.n_sessions() + 1,
            Some("fake"),
            self.curr_session().left.clone(),
            self.curr_session().right.clone(),
        );
        let new_idx = self.n_sessions();
        self.sessions.push(session);
        self.curr_session_idx = new_idx;
    }

    pub(crate) fn run<B: Backend>(
        &'a mut self,
        terminal: &mut Terminal<B>,
    ) -> io::Result<()> {
        loop {
            terminal.draw(|f| self.draw(f))?;
            if let Event::Key(key) = event::read()? {
                match key.code {
                    KeyCode::Char('q') => break,
                    KeyCode::Char('>') => {
                        self.activate_next_session();
                    }
                    KeyCode::Char('<') => {
                        self.activate_prev_session();
                    }
                    KeyCode::Char(c) if c.is_digit(10) => {
                        let id = c.to_digit(10).unwrap() as usize;
                        if id > 0 && id <= self.n_sessions() {
                            self.curr_session_idx = id - 1;
                        }
                    }
                    KeyCode::Char('n') if self.n_sessions() < 9 => {
                        self.add_session();
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    }

    fn draw<B: Backend>(&'a self, frame: &mut Frame<B>) {
        let tabbar = TabBar::new(
            self.sessions.iter().map(|s| s.name).collect::<Vec<&str>>(),
            self.curr_session_idx,
        );
        let session = self.curr_session();
        let tab_sel_hint = format!(
            "{}{}{}",
            "123456789".split_at(self.n_sessions()).0,
            ">",
            "<"
        );
        let cmdbar = CmdBar::new()
            .append_cmd("Tab", tab_sel_hint.as_str())
            .append_cmd("New session", "n")
            .append_cmd("Quit", "q");
        let view = CompositeView::new(Direction::Vertical)
            .add(Box::new(&tabbar))
            .add(Box::new(session))
            .add(Box::new(&cmdbar));
        view.draw(frame, frame.size());
    }
}
