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

use std::io;

use cocomo_core::{DiffItem, FSItem};
use ratatui::{
    DefaultTerminal,
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
};

use crate::{appevent::AppEvent, view::NavigableView};
/// Holds the state and application logic.
use crate::{
    dirview::DirView,
    event::{Event, EventHandler},
    textview::TextView,
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
    TextFile(TextView),
}

impl AppView {
    /// Creates a new `AppView` from the given file system items.
    pub async fn new(
        left_item: &Option<FSItem>,
        right_item: &Option<FSItem>,
    ) -> io::Result<Self> {
        debug_assert!(left_item.is_some() || right_item.is_none());
        match (left_item, right_item) {
            (Some(left), _) => {
                if left.is_dir() {
                    Ok(Self::Dir(DirView::new(left_item, right_item).await?))
                } else {
                    Ok(Self::TextFile(
                        TextView::new(left_item, right_item).await?,
                    ))
                }
            }
            (_, Some(right)) => {
                if right.is_dir() {
                    Ok(Self::Dir(DirView::new(left_item, right_item).await?))
                } else {
                    Ok(Self::TextFile(
                        TextView::new(left_item, right_item).await?,
                    ))
                }
            }
            _ => unreachable!(),
        }
    }

    /// Creates a new `AppView` from the given diff item.
    #[inline(always)]
    pub async fn from_diff_item(diff_item: &DiffItem) -> io::Result<Self> {
        Self::new(&diff_item.left_item, &diff_item.right_item).await
    }

    /// Returns a mutable reference to the view as a [`NavigableView`].
    #[must_use]
    #[inline(always)]
    fn as_nav_view(&mut self) -> &mut dyn NavigableView {
        match self {
            Self::Dir(dir_view) => dir_view,
            Self::TextFile(file_view) => file_view,
        }
    }
}

/// Main application state.
#[derive(Debug)]
pub(crate) struct App {
    /// Flag indicating if the application is running.
    pub running: bool,
    /// Handler for terminal and application events.
    pub events: EventHandler,
    /// Open views (tabs).
    pub views: Vec<AppView>,
    /// Index of the currently active view.
    pub active_view: usize,
    /// Flag to show a confirmation dialog before quitting.
    pub show_quit_confirm: bool,
}

impl App {
    /// Constructs a new instance of [`App`].
    pub(crate) fn new() -> Self {
        Self {
            running: false,
            events: EventHandler::new(),
            views: vec![],
            active_view: 0,
            show_quit_confirm: false,
        }
    }

    /// Returns the active view.
    pub(crate) fn current_view(&self) -> &AppView {
        self.views.get(self.active_view).unwrap()
    }

    /// Returns a mutable reference to the active view.
    pub(crate) fn current_view_mut(&mut self) -> &mut AppView {
        self.views.get_mut(self.active_view).unwrap()
    }

    /// Creates a new app view.
    pub(crate) async fn new_view(
        &mut self,
        left: &Option<FSItem>,
        right: &Option<FSItem>,
    ) -> io::Result<()> {
        let view = AppView::new(left, right).await?;
        self.views.push(view);
        self.active_view = self.views.len() - 1;
        Ok(())
    }

    /// Run the application's main loop.
    ///
    /// # Errors
    ///
    /// Returns an error if terminal drawing or event handling fails.
    pub(crate) async fn run(
        &mut self,
        mut terminal: DefaultTerminal,
    ) -> color_eyre::Result<()> {
        self.running = true;
        while self.running {
            terminal
                .draw(|frame| frame.render_widget(&*self, frame.area()))?;
            match self.events.next().await? {
                Event::Tick => self.tick(),
                Event::Crossterm(event) => match event {
                    crossterm::event::Event::Key(key_event)
                        if key_event.kind
                            == crossterm::event::KeyEventKind::Press =>
                    {
                        self.handle_key_event(key_event)?;
                    }
                    _ => {}
                },
                Event::App(app_event) => {
                    self.handle_app_event(app_event).await?;
                }
            }
        }
        Ok(())
    }

    /// Handles the key events and updates the state of [`App`].
    ///
    /// # Errors
    ///
    /// Returns an error if an application event cannot be sent.
    fn handle_key_event(
        &mut self,
        key_event: KeyEvent,
    ) -> color_eyre::Result<()> {
        if self.show_quit_confirm {
            match key_event.code {
                KeyCode::Char('y') => {
                    self.quit();
                }
                KeyCode::Char('n') | KeyCode::Esc => {
                    self.show_quit_confirm = false;
                }
                _ => {}
            }
            return Ok(());
        }
        match (key_event.code, key_event.modifiers) {
            (KeyCode::Char('q'), KeyModifiers::NONE) => {
                self.events.send(AppEvent::Quit);
            }
            (KeyCode::Char('x'), KeyModifiers::NONE) => {
                self.events.send(AppEvent::CloseTab);
            }
            (KeyCode::Up, KeyModifiers::NONE) => {
                self.events.send(AppEvent::NavigatePrev);
            }
            (KeyCode::Down, KeyModifiers::NONE) => {
                self.events.send(AppEvent::NavigateNext);
            }
            (KeyCode::Home, KeyModifiers::NONE) => {
                self.events.send(AppEvent::NavigateFirst);
            }
            (KeyCode::End, KeyModifiers::NONE) => {
                self.events.send(AppEvent::NavigateLast);
            }
            (KeyCode::Enter, KeyModifiers::NONE) => {
                self.events.send(AppEvent::OpenDiff);
            }
            (KeyCode::Tab, KeyModifiers::NONE) => {
                if !self.views.is_empty() {
                    self.active_view =
                        (self.active_view + 1) % self.views.len();
                }
            }
            (KeyCode::BackTab, KeyModifiers::SHIFT) => {
                if !self.views.is_empty() {
                    self.active_view = if self.active_view == 0 {
                        self.views.len() - 1
                    } else {
                        self.active_view - 1
                    };
                }
            }
            (KeyCode::Char('c'), KeyModifiers::NONE) => {
                self.events.send(AppEvent::Copy);
            }
            (KeyCode::Char('m'), KeyModifiers::NONE) => {
                self.events.send(AppEvent::Move);
            }
            (KeyCode::Char('d'), KeyModifiers::NONE) => {
                self.events.send(AppEvent::Delete);
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

    /// Closes the current tab.
    pub fn close_tab(&mut self) {
        if self.views.len() == 1 {
            self.show_quit_confirm = true;
            return;
        }
        self.views.remove(self.active_view);
        if self.active_view >= self.views.len() {
            self.active_view = self.views.len().saturating_sub(1);
        }
    }

    /// Handles application events from the event channel.
    async fn handle_app_event(
        &mut self,
        app_event: AppEvent,
    ) -> color_eyre::Result<()> {
        match app_event {
            AppEvent::Quit => self.quit(),
            AppEvent::NavigatePrev => {
                let view = self.current_view_mut().as_nav_view();
                view.prev();
            }
            AppEvent::NavigateNext => {
                let view = self.current_view_mut().as_nav_view();
                view.next();
            }
            AppEvent::NavigateFirst => {
                let view = self.current_view_mut().as_nav_view();
                view.home();
            }
            AppEvent::NavigateLast => {
                let view = self.current_view_mut().as_nav_view();
                view.end();
            }
            AppEvent::CloseTab => self.close_tab(),
            AppEvent::OpenDiff => {
                if let AppView::Dir(dir_view) = self.current_view()
                    && let Some(item) = dir_view.current_item()
                {
                    let view = AppView::from_diff_item(item).await?;
                    self.views.push(view);
                    self.active_view = self.views.len() - 1;
                }
            }
            _ => {
                // forward to current app view
                if let AppView::Dir(dir_view) = self.current_view_mut() {
                    dir_view.handle_app_event(app_event).await?;
                    self.events.send(AppEvent::Refresh);
                }
            }
        }
        Ok(())
    }
}
