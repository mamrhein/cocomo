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
use tui::{
    backend::Backend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Span, Spans},
    widgets::{Block, Borders, Tabs},
    Frame, Terminal,
};

use crate::session::Session;

pub(crate) struct App {
    sessions: Vec<Session>,
    curr_session: usize,
}

impl App {
    pub(crate) fn new(session: Session) -> Self {
        Self {
            sessions: vec![session],
            curr_session: 0,
        }
    }

    pub(crate) fn run<B: Backend>(
        &self,
        terminal: &mut Terminal<B>,
    ) -> io::Result<()> {
        loop {
            terminal.draw(|f| self.draw(f))?;

            if let Event::Key(key) = event::read()? {
                match key.code {
                    KeyCode::Char('q') => break,
                    // KeyCode::Right => app.next(),
                    // KeyCode::Left => app.previous(),
                    _ => {}
                }
            }
        }
        Ok(())
    }

    pub(crate) fn draw<B: Backend>(&self, frame: &mut Frame<B>) {
        let size = frame.size();
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints(
                [
                    Constraint::Length(1),
                    Constraint::Min(0),
                    Constraint::Length(1),
                ]
                .as_ref(),
            )
            .split(size);
        let titles = self
            .sessions
            .iter()
            .enumerate()
            .map(|(idx, s)| {
                let name = &s.name;
                Spans::from(vec![
                    Span::styled(name, Style::default()),
                    Span::styled(" ", Style::default()),
                    Span::styled(
                        idx.to_string(),
                        Style::default().fg(Color::Green),
                    ),
                ])
            })
            .collect();
        let tabs = Tabs::new(titles).select(self.curr_session).highlight_style(
            Style::default().add_modifier(Modifier::UNDERLINED),
        );
        frame.render_widget(tabs, chunks[0]);
        frame.render_widget(
            Block::default().title("dummy view").borders(Borders::ALL),
            chunks[1],
        );
        let cmd_bar = Tabs::new(vec![Spans::from("quit <q>")]);
        frame.render_widget(cmd_bar, chunks[2]);
    }
}
