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

use cocomo_core::{
    DirDiff, FSItem, copy_item, delete_item, move_item, rename_item,
};
use ratatui::{
    DefaultTerminal,
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
};

use crate::view::NavigableView;

/// Application events.
#[derive(Clone, Debug)]
#[allow(clippy::large_enum_variant)]
pub(crate) enum AppEvent {
    /// Quit the application.
    Quit,
    /// Navigate the current item of the current view to the previous item.
    NavigatePrev,
    /// Navigate the current item of the current view to the next item.
    NavigateNext,
    /// Navigate the current item of the current view to the first item.
    NavigateFirst,
    /// Navigate the current item of the current view to the last item.
    NavigateLast,
    /// Close the current tab.
    CloseTab,
    /// Open a new comparison view.
    OpenDiff,
    /// Copy the current item to the other side.
    Copy(FSItem, std::path::PathBuf),
    /// Move the current item to the other side.
    Move(FSItem, std::path::PathBuf),
    /// Delete the current item.
    Delete(FSItem),
    /// Rename the current item.
    Rename(FSItem, String),
    /// Refresh the current view.
    Refresh,
}

/// Holds the state and application logic.
use crate::{
    dirview::DirView,
    event::{Event, EventHandler},
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

impl AppView {
    /// Returns a mutable reference to the view as a [`NavigableView`].
    #[must_use]
    #[inline(always)]
    fn as_nav_view(&mut self) -> &mut dyn NavigableView {
        match self {
            Self::Dir(dir_view) => dir_view,
            Self::File(file_view) => file_view,
        }
    }
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
    /// Invariant: !views.is_empty()
    pub views: Vec<AppView>,
    /// Index of the currently active view.
    pub active_view: usize,
    /// Flag to show a confirmation dialog before quitting.
    pub show_quit_confirm: bool,
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
    pub fn current_view(&self) -> &AppView {
        self.views.get(self.active_view).unwrap()
    }

    /// Returns a mutable reference to the active view.
    pub fn current_view_mut(&mut self) -> &mut AppView {
        self.views.get_mut(self.active_view).unwrap()
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
                Event::App(app_event) => {
                    self.handle_app_event(app_event).await;
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
    fn handle_key_events(
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
                let mut copy_op = None;
                if let AppView::Dir(view) = self.current_view()
                    && let Some(i) = view.table_state.borrow().selected()
                    && let Some(item) = view.diff.items.get(i)
                {
                    let r_dir = &view.diff.right_dir;
                    let l_dir = &view.diff.left_dir;
                    if let Some(l) = &item.left_item
                        && r_dir.path().exists()
                    {
                        let dst = r_dir.path().join(l.name());
                        copy_op = Some((l.clone(), dst));
                    } else if let Some(r) = &item.right_item
                        && l_dir.path().exists()
                    {
                        let dst = l_dir.path().join(r.name());
                        copy_op = Some((r.clone(), dst));
                    }
                }
                if let Some((src, dst)) = copy_op {
                    self.events.send(AppEvent::Copy(src, dst));
                }
            }
            (KeyCode::Char('m'), KeyModifiers::NONE) => {
                let mut move_op = None;
                if let AppView::Dir(view) = self.current_view()
                    && let Some(i) = view.table_state.borrow().selected()
                    && let Some(item) = view.diff.items.get(i)
                {
                    let r_dir = &view.diff.right_dir;
                    let l_dir = &view.diff.left_dir;
                    if let Some(l) = &item.left_item
                        && r_dir.path().exists()
                    {
                        let dst = r_dir.path().join(l.name());
                        move_op = Some((l.clone(), dst));
                    } else if let Some(r) = &item.right_item
                        && l_dir.path().exists()
                    {
                        let dst = l_dir.path().join(r.name());
                        move_op = Some((r.clone(), dst));
                    }
                }
                if let Some((src, dst)) = move_op {
                    self.events.send(AppEvent::Move(src, dst));
                }
            }
            (KeyCode::Char('d'), KeyModifiers::NONE) => {
                let mut delete_op = None;
                if let AppView::Dir(view) = self.current_view()
                    && let Some(i) = view.table_state.borrow().selected()
                    && let Some(item) = view.diff.items.get(i)
                {
                    if let Some(l) = &item.left_item {
                        delete_op = Some(l.clone());
                    } else if let Some(r) = &item.right_item {
                        delete_op = Some(r.clone());
                    }
                }
                if let Some(item) = delete_op {
                    self.events.send(AppEvent::Delete(item));
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
    #[allow(clippy::cognitive_complexity)]
    async fn handle_app_event(&mut self, app_event: AppEvent) {
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
                if let AppView::Dir(view) = self.current_view()
                    && let Some(item) = view.current_item()
                {
                    match (&item.left_item, &item.right_item) {
                        (Some(l), Some(r)) => {
                            if l.is_dir() && r.is_dir() {
                                if let Ok(d) =
                                    DirDiff::new(Some(l), Some(r)).await
                                {
                                    self.views
                                        .push(AppView::Dir(DirView::new(d)));
                                    self.active_view = self.views.len() - 1;
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
                                    self.views
                                        .push(AppView::Dir(DirView::new(d)));
                                    self.active_view = self.views.len() - 1;
                                }
                            } else if l.is_file() {
                                let view =
                                    FileView::new(Some(l.clone()), None).await;
                                self.views.push(AppView::File(view));
                                self.active_view = self.views.len() - 1;
                            }
                        }
                        (None, Some(r)) => {
                            if r.is_dir() {
                                if let Ok(d) =
                                    DirDiff::new(None, Some(r)).await
                                {
                                    self.views
                                        .push(AppView::Dir(DirView::new(d)));
                                    self.active_view = self.views.len() - 1;
                                }
                            } else if r.is_file() {
                                let view =
                                    FileView::new(None, Some(r.clone())).await;
                                self.views.push(AppView::File(view));
                                self.active_view = self.views.len() - 1;
                            }
                        }
                        _ => {}
                    }
                }
            }
            AppEvent::Copy(src, dst) => {
                let _ = copy_item(&src, &dst).await;
                self.events.send(AppEvent::Refresh);
            }
            AppEvent::Move(src, dst) => {
                let _ = move_item(&src, &dst).await;
                self.events.send(AppEvent::Refresh);
            }
            AppEvent::Delete(item) => {
                let _ = delete_item(&item).await;
                self.events.send(AppEvent::Refresh);
            }
            AppEvent::Rename(item, new_name) => {
                let _ = rename_item(&item, &new_name).await;
                self.events.send(AppEvent::Refresh);
            }
            AppEvent::Refresh => {
                if let AppView::Dir(dir_view) = self.current_view_mut() {
                    let left = dir_view
                        .diff
                        .left_dir
                        .path()
                        .exists()
                        .then_some(&dir_view.diff.left_dir);
                    let right = dir_view
                        .diff
                        .right_dir
                        .path()
                        .exists()
                        .then_some(&dir_view.diff.right_dir);
                    if let Ok(new_diff) = DirDiff::new(left, right).await {
                        dir_view.diff = new_diff;
                    }
                }
            }
        }
    }
}
