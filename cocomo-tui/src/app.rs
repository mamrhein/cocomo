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

use cocomo_core::FSItem;
use ratatui::{
    DefaultTerminal,
    buffer::Buffer,
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
    layout::{Constraint, Layout, Rect},
    style::{Color, Style},
    widgets::{Block, Clear, Paragraph, Tabs, Widget},
};

use crate::{
    appevent::AppEvent,
    dirview::DirView,
    event::{Event, EventHandler},
    textview::TextView,
    view::NavigableView,
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
pub(crate) type AppView = Box<dyn NavigableView>;

/// Main application state.
#[derive(Debug)]
pub(crate) struct App {
    /// Flag indicating if the application is running.
    running: bool,
    /// Handler for terminal and application events.
    events: EventHandler,
    /// Open views (tabs).
    views: Vec<AppView>,
    /// Index of the currently active view.
    active_view: usize,
    /// Flag to show a confirmation dialog before quitting.
    show_quit_confirm: bool,
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
        left_item: &Option<FSItem>,
        right_item: &Option<FSItem>,
    ) -> io::Result<()> {
        let view: AppView = match (left_item, right_item) {
            (Some(left), _) => {
                if left.is_dir() {
                    Box::new(DirView::new(left_item, right_item).await?)
                } else {
                    Box::new(TextView::new(left_item, right_item).await?)
                }
            }
            (_, Some(right)) => {
                if right.is_dir() {
                    Box::new(DirView::new(left_item, right_item).await?)
                } else {
                    Box::new(TextView::new(left_item, right_item).await?)
                }
            }
            _ => unreachable!(),
        };
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
                self.events.send(AppEvent::OpenView);
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
                let view = self.current_view_mut();
                view.prev();
            }
            AppEvent::NavigateNext => {
                let view = self.current_view_mut();
                view.next();
            }
            AppEvent::NavigateFirst => {
                let view = self.current_view_mut();
                view.home();
            }
            AppEvent::NavigateLast => {
                let view = self.current_view_mut();
                view.end();
            }
            AppEvent::CloseTab => self.close_tab(),
            AppEvent::OpenView => {
                if let Some(item) = self.current_view().current_diff_item() {
                    let left_item = item.left_item.clone();
                    let right_item = item.right_item.clone();
                    self.new_view(&left_item, &right_item).await?;
                };
            }
            _ => {
                // forward to current app view
                return self.current_view_mut().handle_app_event(app_event);
            }
        }
        Ok(())
    }
}

impl Widget for &App {
    /// Renders the user interface widgets.
    fn render(self, area: Rect, buf: &mut Buffer) {
        // Create layout
        let vert_constraints = [
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
        ];
        let [tab_bar, main_view, key_bar] =
            Layout::vertical(vert_constraints).areas(area);

        // Render key hints
        Paragraph::new(
            "q: quit | x: close tab | Enter: open | Tab: switch | ↑/↓: \
             navigate | Home/End: top/bottom | c: copy | m: move | d: delete",
        )
        .left_aligned()
        .render(key_bar, buf);

        let titles: Vec<String> =
            self.views.iter().map(|view| view.title()).collect();

        Tabs::new(titles)
            .select(self.active_view)
            .highlight_style(Style::default().fg(Color::Yellow).bold())
            .divider("|")
            .render(tab_bar, buf);

        // Render current view
        self.current_view().render_ref(main_view, buf);

        if self.show_quit_confirm {
            let area = centered_rect(40, 10, area);
            Clear.render(area, buf);
            let text = "Close last tab and quit? (y/n)";
            Paragraph::new(text)
                .centered()
                .block(Block::bordered())
                .render(area, buf);
        }
    }
}

/// helper function to create a centered rect using up certain % of the
/// available rect `r`
#[allow(clippy::integer_division)]
fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::vertical([
        Constraint::Percentage((100 - percent_y) / 2),
        Constraint::Percentage(percent_y),
        Constraint::Percentage((100 - percent_y) / 2),
    ])
    .split(r);

    Layout::horizontal([
        Constraint::Percentage((100 - percent_x) / 2),
        Constraint::Percentage(percent_x),
        Constraint::Percentage((100 - percent_x) / 2),
    ])
    .split(popup_layout[1])[1]
}
