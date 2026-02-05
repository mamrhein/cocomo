// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

/// Holds the state and application logic.
use crate::event::{AppEvent, Event, EventHandler};
use cocomo_core::FSItem;
use ratatui::{
    DefaultTerminal,
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
};

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
}

impl Default for App {
    fn default() -> Self {
        Self {
            running: true,
            cmp_items: CmpItems::default(),
            events: EventHandler::new(),
        }
    }
}

impl App {
    /// Constructs a new instance of [`App`].
    #[must_use]
    pub fn new(left: Option<FSItem>, right: Option<FSItem>) -> Self {
        Self {
            running: true,
            cmp_items: CmpItems { left, right },
            events: EventHandler::new(),
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
            terminal
                .draw(|frame| frame.render_widget(&self, frame.area()))?;
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
            // KeyCode::Right => self.events.send(AppEvent::Increment),
            // KeyCode::Left => self.events.send(AppEvent::Decrement),
            // Other handlers you could add here.
            _ => {}
        }
        Ok(())
    }

    /// Handles the tick event of the terminal.
    ///
    /// The tick event is where you can update the state of your application with any logic that
    /// needs to be updated at a fixed frame rate. E.g. polling a server, updating an animation.
    #[allow(clippy::unused_self)]
    pub const fn tick(&self) {}

    /// Set running to false to quit the application.
    pub const fn quit(&mut self) {
        self.running = false;
    }
}
