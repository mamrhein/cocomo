// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

use cocomo_core::{DirDiff, FSItem};
use ratatui::{
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
    DefaultTerminal,
};

/// Holds the state and application logic.
use crate::event::{AppEvent, Event, EventHandler};

/// Compare items
#[derive(Debug, Default)]
pub(crate) struct CmpItems {
    pub left: Option<FSItem>,
    pub right: Option<FSItem>,
}

/// Application.
#[derive(Debug)]
pub(crate) struct App {
    /// Is the application running?
    pub running: bool,
    /// Items to compare
    pub cmp_items: CmpItems,
    /// Event handler.
    pub events: EventHandler,
    /// Comparison result
    pub diff: Option<DirDiff>,
    /// Selected item index
    pub selected: usize,
}

impl Default for App {
    fn default() -> Self {
        Self {
            running: true,
            cmp_items: CmpItems::default(),
            events: EventHandler::new(),
            diff: None,
            selected: 0,
        }
    }
}

impl App {
    /// Constructs a new instance of [`App`].
    #[must_use]
    pub async fn new(left: Option<FSItem>, right: Option<FSItem>) -> Self {
        let mut diff = None;
        if let (Some(l), Some(r)) = (&left, &right) {
            if l.is_dir() && r.is_dir() {
                diff = DirDiff::new(l, r).await.ok();
            } else if l.is_file() && r.is_file() {
                if let Ok(item) = cocomo_core::DiffItem::new(&left, &right) {
                    diff = Some(DirDiff {
                        left_dir: l.clone(),
                        right_dir: r.clone(),
                        items: vec![item],
                    });
                }
            }
        }
        Self {
            running: true,
            cmp_items: CmpItems { left, right },
            events: EventHandler::new(),
            diff,
            selected: 0,
        }
    }

    /// Run the application's main loop.
    ///
    /// # Errors
    ///
    /// tbd
    pub async fn run(
        mut self,
        mut terminal: DefaultTerminal,
    ) -> color_eyre::Result<()> {
        while self.running {
            terminal.draw(|frame| frame.render_widget(&self, frame.area()))?;
            match self.events.next().await? {
                Event::Tick => self.tick(),
                Event::Crossterm(event) => match event {
                    crossterm::event::Event::Key(key_event)
                        if key_event.kind
                            == crossterm::event::KeyEventKind::Press =>
                    {
                        self.handle_key_events(key_event)?;
                    }
                    _ => {}
                },
                Event::App(app_event) => match app_event {
                    AppEvent::Quit => self.quit(),
                },
            }
        }
        Ok(())
    }

    /// Handles the key events and updates the state of [`App`].
    ///
    /// # Errors
    ///
    /// tbd
    pub fn handle_key_events(
        &mut self,
        key_event: KeyEvent,
    ) -> color_eyre::Result<()> {
        match key_event.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.events.send(AppEvent::Quit);
            }
            KeyCode::Char('c' | 'C')
                if key_event.modifiers == KeyModifiers::CONTROL =>
            {
                self.events.send(AppEvent::Quit);
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.selected = self.selected.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if let Some(diff) = &self.diff {
                    if !diff.items.is_empty() {
                        self.selected = (self.selected + 1)
                            .min(diff.items.len().saturating_sub(1));
                    }
                }
            }
            KeyCode::Home => {
                self.selected = 0;
            }
            KeyCode::End => {
                if let Some(diff) = &self.diff {
                    if !diff.items.is_empty() {
                        self.selected = diff.items.len().saturating_sub(1);
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Handles the tick event of the terminal.
    ///
    /// The tick event is where you can update the state of your application
    /// with any logic that needs to be updated at a fixed frame rate. E.g.
    /// polling a server, updating an animation.
    #[allow(clippy::unused_self)]
    pub const fn tick(&self) {}

    /// Set running to false to quit the application.
    pub const fn quit(&mut self) {
        self.running = false;
    }
}
