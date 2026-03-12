// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! # Application Module (`app`)
//!
//! This module contains the main application state and logic. It handles
//! events, manages views (tabs), and drives the main loop.

use cocomo_core::{DirDiff, FSItem};
use ratatui::{
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
    DefaultTerminal,
};

use crate::view::NavigableView;
/// Holds the state and application logic.
use crate::{
    dirview::DirView,
    event::{AppEvent, Event, EventHandler},
    fileview::FileView,
};

/// Container for items currently being compared.
#[derive(Debug, Default)]
pub(crate) struct CmpItems {
    /// Left side item.
    pub left: Option<FSItem>,
    /// Right side item.
    pub right: Option<FSItem>,
}

/// Views available in the application.
#[derive(Debug)]
pub(crate) enum AppView {
    /// Directory comparison view.
    Dir(DirView),
    /// File comparison view.
    File(FileView),
}

/// Main application state.
#[derive(Debug)]
pub(crate) struct App {
    /// Flag indicating if the application is running.
    pub running: bool,
    /// The initial items to compare.
    pub cmp_items: CmpItems,
    /// Handler for terminal and application events.
    pub events: EventHandler,
    /// Open views (tabs).
    pub views: Vec<AppView>,
    /// Index of the currently active view.
    pub active_view: usize,
    /// Flag to show a confirmation dialog before quitting.
    pub show_quit_confirm: bool,
}

impl Default for App {
    fn default() -> Self {
        Self {
            running: true,
            cmp_items: CmpItems::default(),
            events: EventHandler::new(),
            views: Vec::new(),
            active_view: 0,
            show_quit_confirm: false,
        }
    }
}

impl App {
    /// Constructs a new instance of [`App`].
    #[must_use]
    pub async fn new(left: Option<FSItem>, right: Option<FSItem>) -> Self {
        let mut views = Vec::new();
        match (left.as_ref(), right.as_ref()) {
            (Some(l), Some(r)) => {
                if l.is_dir() && r.is_dir() {
                    if let Ok(d) = DirDiff::new(Some(l), Some(r)).await {
                        views.push(AppView::Dir(DirView::new(d)));
                    }
                } else if l.is_file() && r.is_file() {
                    let view =
                        FileView::new(Some(l.clone()), Some(r.clone())).await;
                    views.push(AppView::File(view));
                }
            }
            (Some(l), None) => {
                if l.is_dir() {
                    if let Ok(d) = DirDiff::new(Some(l), None).await {
                        views.push(AppView::Dir(DirView::new(d)));
                    }
                } else if l.is_file() {
                    let view = FileView::new(Some(l.clone()), None).await;
                    views.push(AppView::File(view));
                }
            }
            (None, Some(r)) => {
                if r.is_dir() {
                    if let Ok(d) = DirDiff::new(None, Some(r)).await {
                        views.push(AppView::Dir(DirView::new(d)));
                    }
                } else if r.is_file() {
                    let view = FileView::new(None, Some(r.clone())).await;
                    views.push(AppView::File(view));
                }
            }
            _ => {}
        }
        Self {
            running: true,
            cmp_items: CmpItems { left, right },
            events: EventHandler::new(),
            views,
            active_view: 0,
            show_quit_confirm: false,
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
    /// Returns an error if terminal drawing or event handling fails.
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
                    AppEvent::CloseTab => self.close_tab(),
                    AppEvent::OpenDiff(left, right) => {
                        match (left.as_ref(), right.as_ref()) {
                            (Some(l), Some(r)) => {
                                if l.is_dir() && r.is_dir() {
                                    if let Ok(d) =
                                        DirDiff::new(Some(l), Some(r)).await
                                    {
                                        self.views.push(AppView::Dir(
                                            DirView::new(d),
                                        ));
                                        self.active_view =
                                            self.views.len() - 1;
                                    }
                                } else if l.is_file() && r.is_file() {
                                    let view = FileView::new(
                                        Some(l.clone()),
                                        Some(r.clone()),
                                    )
                                    .await;
                                    self.views.push(AppView::File(view));
                                    self.active_view = self.views.len() - 1;
                                }
                            }
                            (Some(l), None) => {
                                if l.is_dir() {
                                    if let Ok(d) =
                                        DirDiff::new(Some(l), None).await
                                    {
                                        self.views.push(AppView::Dir(
                                            DirView::new(d),
                                        ));
                                        self.active_view =
                                            self.views.len() - 1;
                                    }
                                } else if l.is_file() {
                                    let view =
                                        FileView::new(Some(l.clone()), None)
                                            .await;
                                    self.views.push(AppView::File(view));
                                    self.active_view = self.views.len() - 1;
                                }
                            }
                            (None, Some(r)) => {
                                if r.is_dir() {
                                    if let Ok(d) =
                                        DirDiff::new(None, Some(r)).await
                                    {
                                        self.views.push(AppView::Dir(
                                            DirView::new(d),
                                        ));
                                        self.active_view =
                                            self.views.len() - 1;
                                    }
                                } else if r.is_file() {
                                    let view =
                                        FileView::new(None, Some(r.clone()))
                                            .await;
                                    self.views.push(AppView::File(view));
                                    self.active_view = self.views.len() - 1;
                                }
                            }
                            _ => {}
                        }
                    }
                },
            }
        }
        Ok(())
    }

    /// Handles the key events and updates the state of [`App`].
    ///
    /// # Errors
    ///
    /// Returns an error if an application event cannot be sent.
    pub fn handle_key_events(
        &mut self,
        key_event: KeyEvent,
    ) -> color_eyre::Result<()> {
        if self.show_quit_confirm {
            match key_event.code {
                KeyCode::Char('y' | 'Y') => {
                    self.quit();
                }
                KeyCode::Char('n' | 'N') | KeyCode::Esc => {
                    self.show_quit_confirm = false;
                }
                _ => {}
            }
            return Ok(());
        }
        match key_event.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.events.send(AppEvent::Quit);
            }
            KeyCode::Char('x' | 'X') => {
                self.events.send(AppEvent::CloseTab);
            }
            KeyCode::Up | KeyCode::Char('k') => {
                match self.current_view_mut() {
                    Some(AppView::Dir(view)) => {
                        view.move_up();
                    }
                    Some(AppView::File(view)) => {
                        view.move_up();
                    }
                    None => {}
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                match self.current_view_mut() {
                    Some(AppView::Dir(view)) => {
                        view.move_down();
                    }
                    Some(AppView::File(view)) => {
                        view.move_down();
                    }
                    None => {}
                }
            }
            KeyCode::Home => match self.current_view_mut() {
                Some(AppView::Dir(view)) => {
                    view.move_home();
                }
                Some(AppView::File(view)) => {
                    view.move_home();
                }
                None => {}
            },
            KeyCode::End => match self.current_view_mut() {
                Some(AppView::Dir(view)) => {
                    view.move_end();
                }
                Some(AppView::File(view)) => {
                    view.move_end();
                }
                None => {}
            },
            KeyCode::Enter => {
                let mut open_diff = None;
                if let Some(AppView::Dir(view)) = self.current_view_mut() {
                    if let Some(i) = view.table_state.borrow().selected() {
                        if let Some(item) = view.diff.items.get(i) {
                            open_diff = Some((
                                item.left_item.clone(),
                                item.right_item.clone(),
                            ));
                        }
                    }
                }
                if let Some((left, right)) = open_diff {
                    self.events.send(AppEvent::OpenDiff(left, right));
                }
            }
            KeyCode::Tab => {
                if !self.views.is_empty() {
                    self.active_view =
                        (self.active_view + 1) % self.views.len();
                }
            }
            KeyCode::BackTab => {
                if !self.views.is_empty() {
                    self.active_view = if self.active_view == 0 {
                        self.views.len() - 1
                    } else {
                        self.active_view - 1
                    };
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
    pub fn quit(&mut self) {
        self.running = false;
    }

    /// Closes the current tab.
    pub fn close_tab(&mut self) {
        if self.views.is_empty() {
            self.show_quit_confirm = true;
            return;
        }
        if self.views.len() == 1 {
            self.show_quit_confirm = true;
            return;
        }
        self.views.remove(self.active_view);
        if self.active_view >= self.views.len() {
            self.active_view = self.views.len().saturating_sub(1);
        }
    }
}
