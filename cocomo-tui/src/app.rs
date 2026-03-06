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
use crate::{
    dirview::DirView,
    event::{AppEvent, Event, EventHandler},
};

/// Compare items
#[derive(Debug, Default)]
pub(crate) struct CmpItems {
    pub left: Option<FSItem>,
    pub right: Option<FSItem>,
}

/// Views available in the application.
#[derive(Debug)]
pub(crate) enum AppView {
    /// Directory comparison view.
    Dir(DirView),
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
    /// List of views
    pub views: Vec<AppView>,
    /// Index of the active view
    pub active_view: usize,
}

impl Default for App {
    fn default() -> Self {
        Self {
            running: true,
            cmp_items: CmpItems::default(),
            events: EventHandler::new(),
            views: Vec::new(),
            active_view: 0,
        }
    }
}

impl App {
    /// Constructs a new instance of [`App`].
    #[must_use]
    pub async fn new(left: Option<FSItem>, right: Option<FSItem>) -> Self {
        let mut views = Vec::new();
        if let (Some(l), Some(r)) = (&left, &right) {
            let mut diff = None;
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
            if let Some(d) = diff {
                views.push(AppView::Dir(DirView::new(d)));
            }
        }
        Self {
            running: true,
            cmp_items: CmpItems { left, right },
            events: EventHandler::new(),
            views,
            active_view: 0,
        }
    }

    /// Returns the active view.
    pub fn current_view(&self) -> Option<&AppView> {
        self.views.get(self.active_view)
    }

    /// Returns a mutable reference to the active view.
    pub fn current_view_mut(&mut self) -> Option<&mut AppView> {
        self.views.get_mut(self.active_view)
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
                if let Some(AppView::Dir(view)) = self.current_view_mut() {
                    view.move_up();
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if let Some(AppView::Dir(view)) = self.current_view_mut() {
                    view.move_down();
                }
            }
            KeyCode::Home => {
                if let Some(AppView::Dir(view)) = self.current_view_mut() {
                    view.move_home();
                }
            }
            KeyCode::End => {
                if let Some(AppView::Dir(view)) = self.current_view_mut() {
                    view.move_end();
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
